//! JVMTI ClassFileLoadHook — exact OpenJDK 公式 num 準拠 (wave 205 根治)。
//!
//! wave 204 実機検証で判明した構造的欠陥の根治:
//! - vtable index は **公式 jvmti.xml の num − 1** (member 1 = reserved1)。
//!   旧コードは Allocate=46 (正 45)・AddCapabilities を 23/142/22 総当たり・
//!   SetEventNotificationMode を 58/75 (正 1)・コールバックを slot 54 (正 4)
//!   と全体的にずれており、起動経路に達していたら確実に壊れていた。
//! - install を classloader 発見「後」から attach ループ「先頭」へ移動し、
//!   CFLH コールバックが渡してくれる defining loader を**直接捕捉**する
//!   (スレッド歩き探索の完全代替経路。null loader (bootstrap) は拾わない)。
//!   wave 206 教訓: 未アタッチのネイティブスレッドからの GetEnv(JVMTI) は
//!   JVM ごと SIGSEGV する (Temurin 25 実機で確認) ため、install は必ず
//!   **attach 成功済みスレッドから**呼ぶ (HOOK_INSTALLED で冪等)。
//! - wave 206 教訓その2: GetEnv(JVMTI) は毎回**新 JvmtiEnv** を生成するので
//!   先着1個を JVMTI_ENV にキャッシュ・全呼出で共有する。でないと
//!   AddCapabilities が env#A、RetransformClasses が env#B (=cap 未保有)
//!   となり MUST_POSSESS_CAPABILITY (rc=99) で永遠に失敗する (実機確認)。
//! - RetransformClasses でフック install 前にロード済みの対象クラスへ
//!   後追いでパッチを適用する (can_retransform_classes 取得時のみ)。
//!
//! 一次情報: OpenJDK src/hotspot/share/prims/jvmti.xml (wave 205 で引用):
//!   Allocate(num 46) Deallocate(47) GetLoadedClasses(78) GetClassLoaderClasses(79)
//!   IsModifiableClass(45) RetransformClasses(152) RedefineClasses(87)
//!   SetEventCallbacks(122) SetEventNotificationMode(2) GetPotentialCapabilities(140)
//!   AddCapabilities(142) GetCapabilities(89)、イベント CFLH=54、
//!   JVMTI_ENABLE=1。(C index = num − 1)

use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::agent_log::{agent_log, agent_log_step, agent_log_warn};
use crate::jvmti_hook::ClassTransformer;

// ------------------------------------------------------------------
// 公式 spec 由来の index 定数 (C index = num − 1 、単体テストで pin)
// ------------------------------------------------------------------
pub const IDX_ALLOCATE: usize = 45;
pub const IDX_DEALLOCATE: usize = 46;
pub const IDX_GET_LOADED_CLASSES: usize = 77;
pub const IDX_RETRANSFORM_CLASSES: usize = 151;
pub const IDX_REDEFINE_CLASSES: usize = 86;
pub const IDX_SET_EVENT_CALLBACKS: usize = 121;
pub const IDX_SET_EVENT_NOTIFICATION_MODE: usize = 1;
pub const IDX_ADD_CAPABILITIES: usize = 141;
pub const IDX_GET_CAPABILITIES: usize = 88;
pub const IDX_GET_POTENTIAL_CAPABILITIES: usize = 139;
pub const IDX_IS_MODIFIABLE_CLASS: usize = 44;

pub const EVENT_CLASS_FILE_LOAD_HOOK: c_int = 54;
pub const JVMTI_ENABLE: c_int = 1;
/// ClassFileLoadHook コールバックの struct スロット (VMInit/VMDeath/ThreadStart/ThreadEnd の次)。
pub const CALLBACK_SLOT_CLASS_FILE_LOAD_HOOK: usize = 4;

const JVMTI_VERSION_1_2: c_int = 0x30010002;
/// JNIInvokeInterface::GetEnv (reserved0-2, Destroy, Attach, Detach, GetEnv=idx6)。
const IDX_JNI_GET_ENV: usize = 6;
/// JNINativeInterface::NewGlobalRef (idx 21)。
const IDX_JNI_NEW_GLOBAL_REF: usize = 21;

static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
/// jvmtiEnv 共有キャッシュ (wave 206 実機で判明・HA-4):
/// GetEnv(JVMTI) は**呼出ごとに新しい JvmtiEnv を生成する** (HotSpot 一次情報:
/// JvmtiExport::get_jvmti_interface が LIVE 相で
/// `JvmtiEnv::create_a_jvmti(version)` を毎回 new して返す)。
/// capability は env 単位で管理されるため、install 時の env (env#A) に
/// AddCapabilities しても、後続呼出で取り直した別 env (env#B) の
/// RetransformClasses は MUST_POSSESS_CAPABILITY (rc=99) を返す —
/// つまり env をキャッシュして全 JVMTI 呼出で共有しないと retransform /
/// GetLoadedClasses 系は永遠に動かない。Temurin 25 で機械確認済。
static JVMTI_ENV: AtomicUsize = AtomicUsize::new(0);
/// CFLH で捕捉した game loader の global ref (jobject アドレス、0=未捕捉)。
static CAPTURED_LOADER: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_LOGGED: AtomicBool = AtomicBool::new(false);

/// 捕捉済み game loader のグローバル参照 (JNIEnv 上で使う生 jobject 値)。
pub fn captured_game_loader_raw() -> Option<usize> {
    let v = CAPTURED_LOADER.load(Ordering::SeqCst);
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

/// JvmtiCapabilities — u32 × 語の 1 ビット並び (公式 bit 位置)。
/// can_redefine_classes=bit9 / can_generate_all_class_hook_events=bit26 /
/// can_retransform_classes=bit37 / can_retransform_any_class=bit38 /
/// can_generate_early_class_hook_events=bit42 (jvmti.xml capabilityfield 順序)。
#[repr(C)]
pub struct JvmtiCapabilities {
    pub words: [u32; 4],
}

impl JvmtiCapabilities {
    pub fn new() -> Self {
        Self { words: [0; 4] }
    }

    pub fn set_bit(&mut self, bit: usize) {
        self.words[bit / 32] |= 1u32 << (bit % 32);
    }

    pub fn get_bit(&self, bit: usize) -> bool {
        self.words[bit / 32] & (1u32 << (bit % 32)) != 0
    }

    pub const BIT_CAN_REDEFINE_CLASSES: usize = 9;
    pub const BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS: usize = 26;
    pub const BIT_CAN_RETRANSFORM_CLASSES: usize = 37;
    pub const BIT_CAN_RETRANSFORM_ANY_CLASS: usize = 38;
    pub const BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS: usize = 42;
}

type AllocateFn = unsafe extern "system" fn(*mut c_void, i64, *mut *mut c_uchar) -> c_int;
type AddCapabilitiesFn = unsafe extern "system" fn(*mut c_void, *const JvmtiCapabilities) -> c_int;
type GetCapabilitiesFn = unsafe extern "system" fn(*mut c_void, *mut JvmtiCapabilities) -> c_int;
type SetEventCallbacksFn =
    unsafe extern "system" fn(*mut c_void, *const JvmtiEventCallbacks, c_int) -> c_int;
type SetEventNotificationModeFn =
    unsafe extern "system" fn(*mut c_void, c_int, c_int, *mut c_void) -> c_int;
type RetransformClassesFn =
    unsafe extern "system" fn(*mut c_void, c_int, *const *mut c_void) -> c_int;
type IsModifiableClassFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> c_int;

#[repr(C)]
pub struct JvmtiEventCallbacks {
    data: [usize; 16],
}

impl JvmtiEventCallbacks {
    fn zeroed() -> Self {
        Self { data: [0; 16] }
    }

    fn set_class_file_load_hook(&mut self, cb: *const c_void) {
        self.data[CALLBACK_SLOT_CLASS_FILE_LOAD_HOOK] = cb as usize;
    }
}

type ClassFileLoadHookFn = unsafe extern "system" fn(
    jvmti_env: *mut c_void,
    jni_env: *mut c_void,
    class_being_redefined: *mut c_void,
    loader: *mut c_void,
    name: *const c_char,
    protection_domain: *mut c_void,
    class_data_len: c_int,
    class_data: *const c_uchar,
    new_class_data_len: *mut c_int,
    new_class_data: *mut *mut c_uchar,
);

unsafe extern "system" fn class_file_load_hook(
    jvmti_env: *mut c_void,
    jni_env: *mut c_void,
    _class_being_redefined: *mut c_void,
    loader: *mut c_void,
    name: *const c_char,
    _protection_domain: *mut c_void,
    class_data_len: c_int,
    class_data: *const c_uchar,
    new_class_data_len: *mut c_int,
    new_class_data: *mut *mut c_uchar,
) {
    if name.is_null() || class_data.is_null() || class_data_len <= 0 {
        return;
    }
    let c_name = std::ffi::CStr::from_ptr(name);
    let class_name = match c_name.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };

    // game loader 捕捉: net/minecraft 系クラスの defining loader を
    // (イベントが渡してくれる) 直接 global ref 化する。スレッド探索の完全代替。
    if !loader.is_null() && class_name.starts_with("net/minecraft/") {
        if CAPTURED_LOADER
            .compare_exchange(0, loader as usize, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let g = jni_new_global_ref(jni_env, loader);
            if g != 0 {
                CAPTURED_LOADER.store(g, Ordering::SeqCst);
                if !CAPTURE_LOGGED.swap(true, Ordering::SeqCst) {
                    agent_log(&format!(
                        "[JVMTI] game ClassLoader captured via CFLH (first class: {})",
                        class_name
                    ));
                }
            } else {
                // global ref 化失敗 — 捕捉解除して次のクラスで再試行
                CAPTURED_LOADER.store(0, Ordering::SeqCst);
            }
        }
    }

    if !rsift_parser::BytecodePatcher::is_target_class(class_name) {
        return;
    }

    let slice = std::slice::from_raw_parts(class_data, class_data_len as usize);
    let Some(patched) = ClassTransformer::on_class_load(class_name, slice) else {
        return;
    };
    if patched.is_empty() || patched.len() == slice.len() && patched.as_slice() == slice {
        return;
    }

    let allocate: AllocateFn = match jvmti_fn(jvmti_env, IDX_ALLOCATE) {
        Some(f) => std::mem::transmute(f),
        None => return,
    };
    let mut buf: *mut c_uchar = ptr::null_mut();
    let err = allocate(jvmti_env, patched.len() as i64, &mut buf);
    if err != 0 || buf.is_null() {
        return;
    }
    std::ptr::copy_nonoverlapping(patched.as_ptr(), buf, patched.len());
    *new_class_data_len = patched.len() as c_int;
    *new_class_data = buf;
}

unsafe fn jni_new_global_ref(jni_env: *mut c_void, obj: *mut c_void) -> usize {
    if jni_env.is_null() || obj.is_null() {
        return 0;
    }
    let functions = *(jni_env as *mut *mut *mut c_void);
    if functions.is_null() {
        return 0;
    }
    type NewGlobalRefFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void;
    let f: NewGlobalRefFn = std::mem::transmute(*functions.add(IDX_JNI_NEW_GLOBAL_REF));
    let g = f(jni_env, obj);
    g as usize
}

unsafe fn jvmti_fn(env: *mut c_void, index: usize) -> Option<*mut c_void> {
    if env.is_null() {
        return None;
    }
    let functions = *(env as *mut *mut *mut c_void);
    if functions.is_null() {
        return None;
    }
    let f = *functions.add(index);
    if f.is_null() {
        None
    } else {
        Some(f)
    }
}

/// JVM から jvmtiEnv を取る。**先着1個の env を全呼出で共有**する
/// (GetEnv が毎回新 env を生成する事実 (JVMTI_ENV コメント参照) への根治)。
/// 未アタッチのネイティブスレッドから呼ぶと JVM ごと SIGSEGV するので、
/// 呼出元は必ず attach 済みスレッドであること (install 条件と同じ)。
unsafe fn get_jvmti_env(java_vm: *mut c_void) -> Option<*mut c_void> {
    let cached = JVMTI_ENV.load(Ordering::SeqCst);
    if cached != 0 {
        return Some(cached as *mut c_void);
    }
    let invoke = *(java_vm as *mut *mut *mut c_void);
    if invoke.is_null() {
        return None;
    }
    type GetEnvFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int;
    let get_env: GetEnvFn = std::mem::transmute(*invoke.add(IDX_JNI_GET_ENV));
    let mut jvmti: *mut c_void = ptr::null_mut();
    let rc = get_env(java_vm, &mut jvmti, JVMTI_VERSION_1_2);
    if rc == 0 && !jvmti.is_null() {
        // 先着の env を共有キャッシュへ。CAS 敗者は自分の生成物を捨てて勝者の
        // env を返す (env は独立オブジェクトで捨てても害なし — capability は
        // 勝者 env 側で install 済みなので全呼出が能力を共有できる)。
        let _ = JVMTI_ENV.compare_exchange(0, jvmti as usize, Ordering::SeqCst, Ordering::SeqCst);
        let shared = JVMTI_ENV.load(Ordering::SeqCst);
        if shared != jvmti as usize {
            agent_log_warn(
                "jvmti",
                "GetEnv race detected — dropping duplicate env (harmless, shared env wins)",
            );
        }
        return Some(shared as *mut c_void);
    }
    None
}

/// ClassFileLoadHook を install。`java_vm` は生存中の JavaVM 実体 (deferred
/// init または Agent_OnLoad のもの)。**attach 済みスレッドから呼ぶこと** —
/// 未アタッチのネイティブスレッドからの GetEnv(JVMTI) は HotSpot が現在
/// スレッドを null デリファレンスして JVM ごと SIGSEGV する (wave 206
/// 実機確認: si_addr=0x52c)。HOOK_INSTALLED で冪等 (先着1回)。
pub unsafe fn install_class_file_load_hook(java_vm: *mut c_void) -> bool {
    if HOOK_INSTALLED.load(Ordering::SeqCst) {
        return true;
    }
    if java_vm.is_null() {
        return false;
    }

    unsafe {
        let Some(jvmti) = get_jvmti_env(java_vm) else {
            agent_log_warn("jvmti", "GetEnv(jvmti) failed");
            return false;
        };

        // wave 206 実機検証で判明 (HA-3): 1 つでも付与不能な capability を
        // 含めると AddCapabilities は JVMTI_ERROR_NOT_AVAILABLE (98) で全滅
        // し何も付与されない (Temurin 25 で live 相から early 系 bit を請求
        // して全滅を観測 → RetransformClasses も rc=99 FAIL に連鎖)。
        // 正攻法: GetPotentialCapabilities (idx 139) で取得可能集合を引き、
        // 「必要 ∩ 取得可能」だけを請求する (early 系は OnLoad 相でしか
        // 取得できない JVM では自動的に外れる)。
        type GetPotentialCapabilitiesFn =
            unsafe extern "system" fn(*mut c_void, *mut JvmtiCapabilities) -> c_int;
        let mut potential = JvmtiCapabilities::new();
        if let Some(f) = jvmti_fn(jvmti, IDX_GET_POTENTIAL_CAPABILITIES) {
            let get_pot: GetPotentialCapabilitiesFn = std::mem::transmute(f);
            let rc = get_pot(jvmti, &mut potential);
            if rc != 0 {
                agent_log_warn(
                    "jvmti",
                    &format!(
                        "GetPotentialCapabilities rc={} — request を素通しします",
                        rc
                    ),
                );
                // 取得不能のまま進める場合、念のため全 bit potential とみなす
                // (従来動作と同じ請求を試み、rc をログに残す)。
                potential.words = [u32::MAX; 4];
            }
        } else {
            agent_log_warn(
                "jvmti",
                "GetPotentialCapabilities fn not resolved — request を素通しします",
            );
            potential.words = [u32::MAX; 4];
        }

        let mut desired = JvmtiCapabilities::new();
        desired.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS);
        desired.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS);
        desired.set_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES);
        desired.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES);
        desired.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS);
        let mut caps = JvmtiCapabilities::new();
        for i in 0..4 {
            caps.words[i] = desired.words[i] & potential.words[i];
        }
        agent_log_step(
            "jvmti",
            &format!(
                "AddCapabilities request all_hook={} early_hook={} redefine={} retransform={} retransform_any={} (potential ∩ desired)",
                caps.get_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS),
                caps.get_bit(JvmtiCapabilities::BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS),
                caps.get_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES),
                caps.get_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES),
                caps.get_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS),
            ),
        );
        let add: AddCapabilitiesFn = match jvmti_fn(jvmti, IDX_ADD_CAPABILITIES) {
            Some(f) => std::mem::transmute(f),
            None => {
                agent_log_warn("jvmti", "AddCapabilities fn not resolved");
                return false;
            }
        };
        let rc = add(jvmti, &caps);
        if rc != 0 {
            // 部分付与でも CFLH の install 自体には進む (retransform 後追いは
            // 付与時のみ機能する。rc=99 連鎖は post_bridge_boot が rc を必ず
            // ログに残すため診断可能)。
            agent_log_warn(
                "jvmti",
                &format!(
                    "AddCapabilities rc={} — 未取得の能力あり。CFLH install は続行します",
                    rc
                ),
            );
        } else {
            agent_log_step("jvmti", "AddCapabilities rc=0 (全請求受理)");
        }

        // 実際に認められた能力を確認ログ (実機診断の一次情報)。
        if let Some(f) = jvmti_fn(jvmti, IDX_GET_CAPABILITIES) {
            let get: GetCapabilitiesFn = std::mem::transmute(f);
            let mut granted = JvmtiCapabilities::new();
            let rc2 = get(jvmti, &mut granted);
            agent_log_step(
                "jvmti",
                &format!(
                    "GetCapabilities rc={} all_hook={} early_hook={} redefine={} retransform={} retransform_any={}",
                    rc2,
                    granted.get_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS),
                    granted.get_bit(JvmtiCapabilities::BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS),
                    granted.get_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES),
                    granted.get_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES),
                    granted.get_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS),
                ),
            );
        }

        let mut callbacks = JvmtiEventCallbacks::zeroed();
        callbacks.set_class_file_load_hook(class_file_load_hook as *const c_void);
        let set_cb: SetEventCallbacksFn = match jvmti_fn(jvmti, IDX_SET_EVENT_CALLBACKS) {
            Some(f) => std::mem::transmute(f),
            None => {
                agent_log_warn("jvmti", "SetEventCallbacks fn not resolved");
                return false;
            }
        };
        let size = std::mem::size_of::<JvmtiEventCallbacks>() as c_int;
        let rc = set_cb(jvmti, &callbacks, size);
        if rc != 0 {
            agent_log_warn("jvmti", &format!("SetEventCallbacks rc={}", rc));
            return false;
        }

        let set_mode: SetEventNotificationModeFn =
            match jvmti_fn(jvmti, IDX_SET_EVENT_NOTIFICATION_MODE) {
                Some(f) => std::mem::transmute(f),
                None => {
                    agent_log_warn("jvmti", "SetEventNotificationMode fn not resolved");
                    return false;
                }
            };
        let rc = set_mode(
            jvmti,
            JVMTI_ENABLE,
            EVENT_CLASS_FILE_LOAD_HOOK,
            ptr::null_mut(),
        );
        if rc != 0 {
            agent_log_warn("jvmti", &format!("SetEventNotificationMode rc={}", rc));
            return false;
        }
        HOOK_INSTALLED.store(true, Ordering::SeqCst);
        agent_log("[JVMTI] ClassFileLoadHook enabled (exact-spec indices)");
        true
    }
}

/// フック install 前にロード済みのクラスへ後追いパッチ。`java_vm` は生存中
/// JavaVM。classes は jclass 配列 (loadClass 等で取得済み)。rc と件数を記録。
pub unsafe fn retransform_classes(java_vm: *mut c_void, classes: &[*mut c_void]) -> i32 {
    if classes.is_empty() {
        return 0;
    }
    unsafe {
        let Some(jvmti) = get_jvmti_env(java_vm) else {
            return -1;
        };
        let rt: RetransformClassesFn = match jvmti_fn(jvmti, IDX_RETRANSFORM_CLASSES) {
            Some(f) => std::mem::transmute(f),
            None => return -1,
        };
        rt(jvmti, classes.len() as c_int, classes.as_ptr())
    }
}

type GetLoadedClassesFn =
    unsafe extern "system" fn(*mut c_void, *mut c_int, *mut *mut *mut c_void) -> c_int;
type DeallocateFn = unsafe extern "system" fn(*mut c_void, *mut c_uchar) -> c_int;

/// JVMTI GetLoadedClasses (idx 77) — ロード済み全クラスの jclass 配列。
/// 返る jclass は呼出スレッドの JNI local ref (attach 済みスレッドで使うこと)。
/// JVMTI 側の配列バッファは本関数内で Deallocate 済み (要素の local ref は無関係)。
/// スレッド歩き探索の決定的代替戦略 (wave 205) で screen_inject から使う。
pub unsafe fn get_loaded_classes(java_vm: *mut c_void) -> Option<Vec<*mut c_void>> {
    unsafe {
        let jvmti = get_jvmti_env(java_vm)?;
        let f: GetLoadedClassesFn = std::mem::transmute(jvmti_fn(jvmti, IDX_GET_LOADED_CLASSES)?);
        let mut count: c_int = 0;
        let mut classes: *mut *mut c_void = ptr::null_mut();
        let rc = f(jvmti, &mut count, &mut classes);
        if rc != 0 || classes.is_null() || count <= 0 {
            if !classes.is_null() {
                if let Some(df) = jvmti_fn(jvmti, IDX_DEALLOCATE) {
                    let dealloc: DeallocateFn = std::mem::transmute(df);
                    let _ = dealloc(jvmti, classes as *mut c_uchar);
                }
            }
            return None;
        }
        let out: Vec<*mut c_void> = std::slice::from_raw_parts(classes, count as usize).to_vec();
        if let Some(df) = jvmti_fn(jvmti, IDX_DEALLOCATE) {
            let dealloc: DeallocateFn = std::mem::transmute(df);
            let _ = dealloc(jvmti, classes as *mut c_uchar);
        }
        Some(out)
    }
}

/// IsModifiableClass (診断ログ用)。
pub unsafe fn is_modifiable_class(java_vm: *mut c_void, jclass: *mut c_void) -> Option<bool> {
    unsafe {
        let jvmti = get_jvmti_env(java_vm)?;
        let f: IsModifiableClassFn = std::mem::transmute(jvmti_fn(jvmti, IDX_IS_MODIFIABLE_CLASS)?);
        Some(f(jvmti, jclass) != 0)
    }
}

/// Fallback: transform via Rust when Java asks (safe after bootstrap).
pub fn transform_bytes(class_name: &str, data: &[u8]) -> Option<Vec<u8>> {
    ClassTransformer::on_class_load(class_name, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtable_indices_match_openjdk_jvmti_xml_nums() {
        // 一次情報: OpenJDK jvmti.xml num 値 − 1。回帰ピン。
        assert_eq!(IDX_ALLOCATE, 46 - 1);
        assert_eq!(IDX_DEALLOCATE, 47 - 1);
        assert_eq!(IDX_GET_LOADED_CLASSES, 78 - 1);
        assert_eq!(IDX_RETRANSFORM_CLASSES, 152 - 1);
        assert_eq!(IDX_REDEFINE_CLASSES, 87 - 1);
        assert_eq!(IDX_SET_EVENT_CALLBACKS, 122 - 1);
        assert_eq!(IDX_SET_EVENT_NOTIFICATION_MODE, 2 - 1);
        assert_eq!(IDX_ADD_CAPABILITIES, 142 - 1);
        assert_eq!(IDX_GET_CAPABILITIES, 89 - 1);
        assert_eq!(IDX_GET_POTENTIAL_CAPABILITIES, 140 - 1);
        assert_eq!(IDX_IS_MODIFIABLE_CLASS, 45 - 1);
        assert_eq!(EVENT_CLASS_FILE_LOAD_HOOK, 54);
        assert_eq!(CALLBACK_SLOT_CLASS_FILE_LOAD_HOOK, 4);
        assert_eq!(IDX_JNI_GET_ENV, 6);
        assert_eq!(IDX_JNI_NEW_GLOBAL_REF, 21);
    }

    #[test]
    fn capability_bits_match_openjdk_capabilityfield_order() {
        // jvmti.xml capabilityfield 文書順 (bit 0 = can_tag_objects …)。
        let mut c = JvmtiCapabilities::new();
        c.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS); // 26 → word0 bit26
        c.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES); // 37 → word1 bit5
        c.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS); // 38 → word1 bit6
        c.set_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES); // 9 → word0 bit9
        c.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS); // 42 → word1 bit10
        assert_eq!(c.words[0], (1 << 9) | (1 << 26));
        assert_eq!(c.words[1], (1 << 5) | (1 << 6) | (1 << 10));
        assert_eq!(c.words[2], 0);
        assert!(c.get_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS));
        assert!(!c.get_bit(41), "bit41 (early_vmstart) は未設定");
    }

    #[test]
    fn captured_loader_zero_means_none() {
        // 環境に左右されない不変条件 (初期状態・0 = 未捕捉)。
        let v = CAPTURED_LOADER.load(Ordering::SeqCst);
        if v == 0 {
            assert!(captured_game_loader_raw().is_none());
        } else {
            assert!(captured_game_loader_raw().is_some());
        }
    }
}
