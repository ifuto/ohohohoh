//! JVMTI ClassFileLoadHook — exact OpenJDK 公式 num 準拠 (wave 205 根治)。
//!
//! wave 204 実機検証で判明した構造的欠陥の根治:
//! - vtable index は **公式 jvmti.xml の num − 1** (member 1 = reserved1)。
//!   旧コードは Allocate=46 (正 45)・AddCapabilities を 23/142/22 総当たり・
//!   SetEventNotificationMode を 58/75 (正 1)・コールバックを slot 54 (正 4)
//!   と全体的にずれており、起動経路に達していたら確実に壊れていた。
//! - install を classloader 発見「後」から「**attach 前 (最速)**」へ移動し、
//!   CFLH コールバックが渡してくれる defining loader を**直接捕捉**する
//!   (スレッド歩き探索の完全代替経路。null loader (bootstrap) は拾わない)。
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

/// JVM から jvmtiEnv を取る。
unsafe fn get_jvmti_env(java_vm: *mut c_void) -> Option<*mut c_void> {
    let invoke = *(java_vm as *mut *mut *mut c_void);
    if invoke.is_null() {
        return None;
    }
    type GetEnvFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int;
    let get_env: GetEnvFn = std::mem::transmute(*invoke.add(IDX_JNI_GET_ENV));
    let mut jvmti: *mut c_void = ptr::null_mut();
    let rc = get_env(java_vm, &mut jvmti, JVMTI_VERSION_1_2);
    if rc == 0 && !jvmti.is_null() {
        return Some(jvmti);
    }
    None
}

/// ClassFileLoadHook を install。`java_vm` は生存中の JavaVM 実体 (deferred
/// init または Agent_OnLoad のもの)。attach 前に呼ぶこと (最速で loader 捕捉)。
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

        // 必要能力を 1 回の AddCapabilities で請求 (rc を必ず記録)。
        let mut caps = JvmtiCapabilities::new();
        caps.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_ALL_CLASS_HOOK_EVENTS);
        caps.set_bit(JvmtiCapabilities::BIT_CAN_GENERATE_EARLY_CLASS_HOOK_EVENTS);
        caps.set_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES);
        caps.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES);
        caps.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS);
        let add: AddCapabilitiesFn = match jvmti_fn(jvmti, IDX_ADD_CAPABILITIES) {
            Some(f) => std::mem::transmute(f),
            None => {
                agent_log_warn("jvmti", "AddCapabilities fn not resolved");
                return false;
            }
        };
        let rc = add(jvmti, &caps);
        agent_log_step("jvmti", &format!("AddCapabilities rc={}", rc));

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
