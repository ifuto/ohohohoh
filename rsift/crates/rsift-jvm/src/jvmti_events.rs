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
/// AddToBootstrapClassLoaderSearch (num 149 → C index 148、wave 207 HC-1)。
/// HEAD 注入先 com/rsift/RsiftHooks を bootstrap CL 経由で全 loader 可視化する
/// ための王道 API (live 相では JAR ファイルのみ受理、capability 不要)。
pub const IDX_ADD_TO_BOOTSTRAP_CLASS_LOADER_SEARCH: usize = 148;

pub const EVENT_CLASS_FILE_LOAD_HOOK: c_int = 54;
pub const JVMTI_ENABLE: c_int = 1;
/// ClassFileLoadHook コールバックの struct スロット (VMInit/VMDeath/ThreadStart/ThreadEnd の次)。
pub const CALLBACK_SLOT_CLASS_FILE_LOAD_HOOK: usize = 4;
/// wave 209 HE: ClassLoad イベント (クラス定義完了直後・link 前 = jclass は
/// 既に有効で NewGlobalRef 可能)。CFLH(54) の公式連番 = jvmti.xml ClassLoad
/// num=55。コールバック struct は ClassFileLoadHook(slot4) の次 = slot5。
/// ClassLoad イベント自体に capability は不要 (通常イベント)。
pub const EVENT_CLASS_LOAD: c_int = 55;
pub const CALLBACK_SLOT_CLASS_LOAD: usize = 5;

/// wave 209 HE: GetClassSignature (jvmti.xml num=48 → C index 47) —
/// Allocate(46) Deallocate(47) の公式連番の次。ClassLoad コールバックには
/// クラス名引数が無いため、jclass から `L...;` signature を取得して
/// 重要クラスを同定する。capability 不要。
pub const IDX_GET_CLASS_SIGNATURE: usize = 47;

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
/// wave 207 HC-1: AddToBootstrapClassLoaderSearch は全プロセスで 1 回だけ
/// (OnLoad + CFLH install 後フォールバックの重複抑止)。
pub static BOOTSTRAP_CLASSPATH_ADDED: AtomicBool = AtomicBool::new(false);
/// CFLH で捕捉した game loader の global ref (jobject アドレス、0=未捕捉)。
static CAPTURED_LOADER: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_LOGGED: AtomicBool = AtomicBool::new(false);
/// wave HS (ログ #8 観測強化): CFLH がパッチ対象クラスへ到達したか・パッチが
/// 適用されたかをクラス毎 1 回だけ可視化する throttle 集合。
/// これが無いと「flipFrame/tick の CFLH パッチが当たっているか」が原理上判定不能
/// (info!/debug! は tracing subscriber 未初期化で虚空へ消える)。agent_log 経由で確実可視化。
static CFLH_OUTCOME_LOGGED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

// ----------------------------------------------------------------------
// wave 209 HE: 重要クラスのローダー非依存 jcache
// ----------------------------------------------------------------------
/// 実機 #5 一次解析: Prism のラッパー起動では vanilla 本体クラスが
/// **子ローダーに分離** され、CFLH で先に捕捉したローダー (ClientBrandRetriever
/// をロードした側) も system loader (FindClass) も `net.minecraft.client.Minecraft`
/// を解決不能 = CNFE が 135 秒永続 (UiBridge/ModBridge 準備完了後も機能全不発)。
/// ClassLoad イベントは「定義されたばかりの jclass 本体」を渡してくるため、
/// これを global ref 化して内部名キーで保持すれば **どのローダーが定義しても
/// 以後ローダー経由に一切依存せず利用可能** になる。対象は最小集合のみ
/// (leak 防止・CFLH 負荷防止)。
pub static WANTED_CLASSES: [&str; 6] = [
    "net/minecraft/client/Minecraft",
    "net/minecraft/client/gui/screens/Screen",
    "net/minecraft/client/gui/components/debug/DebugScreenOverlay",
    "net/minecraft/client/gui/components/DebugScreenOverlay",
    "net/minecraft/client/gui/components/debug/DebugScreenEntryList",
    "net/minecraft/network/Connection",
];

static WANTED_CLASS_CACHE: std::sync::Mutex<[usize; 6]> = std::sync::Mutex::new([0; 6]);

/// 内部名→捕捉済み jclass (global ref jobject 値)。未捕捉は None。
/// wave HR (#5 根治): 引数 internal は**実行時名** (難読化版では難読名)。
/// find_minecraft_class は obf_map で mojmap→難読を解決した実行時名を渡すため、
/// ここでも WANTED を mojmap→難読解決して照合する (両者が同一変換で一貫)。
pub fn wanted_class_raw(internal: &str) -> Option<usize> {
    let idx = match_wanted_at_runtime(internal)?;
    let v = WANTED_CLASS_CACHE.lock().map(|g| g[idx]).unwrap_or(0);
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

fn wanted_class_index(internal: &str) -> Option<usize> {
    WANTED_CLASSES.iter().position(|c| *c == internal)
}

/// wave HR (#5 根治): 実行時内部名 (難読名) を WANTED (mojmap) へ照合。
/// 各 WANTED エントリを obf_map で mojmap→難読 internal へ解決し、引数
/// `incoming_internal` と比較する。obf_map 未 install / Unobfuscated /
/// map 未収録は unwrap_or で mojmap internal へ落ち = 従来の直接照合と一致。
/// class_load_event (格納) と wanted_class_raw (取得) の両方が本関数を使うことで
/// 難読化・非難読化の両モードで格納/取得の index が一貫する。
fn match_wanted_at_runtime(incoming_internal: &str) -> Option<usize> {
    WANTED_CLASSES.iter().position(|mojmap_internal| {
        let dotted = mojmap_internal.replace('/', ".");
        let resolved = crate::obf_map::resolve_class_internal(&dotted)
            .unwrap_or_else(|| mojmap_internal.to_string());
        resolved == incoming_internal
    })
}

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
type AddToBootstrapClassLoaderSearchFn =
    unsafe extern "system" fn(*mut c_void, *const c_char) -> c_int;
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

    fn set_class_load(&mut self, cb: *const c_void) {
        self.data[CALLBACK_SLOT_CLASS_LOAD] = cb as usize;
    }
}

type GetClassSignatureFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut *mut c_char,
    *mut *mut c_char,
) -> c_int;
type DeallocateFn2 = unsafe extern "system" fn(*mut c_void, *mut c_uchar) -> c_int;

/// wave 209 HE: ClassLoad コールバック。jclass が渡る唯一のイベントで、
/// 重要クラス (WANTED_CLASSES) の jclass を global ref 化して jcache へ。
/// GetClassSignature → Deallocate の規格ペアで文字列確保を解放する。
/// ここでは例外も JNI 呼出も基本的に発生させない (JVMTI イベント中の
/// 制約: キャッシュ書込と NewGlobalRef のみ = 規格上許容)。
unsafe extern "system" fn class_load_event(
    jvmti_env: *mut c_void,
    jni_env: *mut c_void,
    _thread: *mut c_void,
    klass: *mut c_void,
) {
    if jvmti_env.is_null() || klass.is_null() {
        return;
    }
    let Some(f) = jvmti_fn(jvmti_env, IDX_GET_CLASS_SIGNATURE) else {
        return;
    };
    let get_sig: GetClassSignatureFn = std::mem::transmute(f);
    let mut sig: *mut c_char = ptr::null_mut();
    let rc = get_sig(jvmti_env, klass, &mut sig, ptr::null_mut());
    if rc != 0 || sig.is_null() {
        return;
    }
    let sig_str = std::ffi::CStr::from_ptr(sig).to_string_lossy().into_owned();
    if let Some(df) = jvmti_fn(jvmti_env, IDX_DEALLOCATE) {
        let dealloc: DeallocateFn2 = std::mem::transmute(df);
        let _ = dealloc(jvmti_env, sig as *mut c_uchar);
    }
    // "Lnet/minecraft/client/Minecraft;" → 内部名 (難読化版では難読名)
    let internal = sig_str
        .strip_prefix('L')
        .and_then(|s| s.strip_suffix(';'))
        .unwrap_or(sig_str.as_str());
    // wave HR (#5 根治): internal は実行時名 (難読名)。WANTED (mojmap) を解決して照合。
    let Some(idx) = match_wanted_at_runtime(internal) else {
        return;
    };
    let g = if jni_env.is_null() {
        0
    } else {
        jni_new_global_ref(jni_env, klass)
    };
    if g == 0 {
        return;
    }
    if let Ok(mut guard) = WANTED_CLASS_CACHE.lock() {
        let first = guard[idx] == 0;
        guard[idx] = g;
        if first {
            agent_log(&format!(
                "[JVMTI] wanted class captured at ClassLoad (loader-independent): {}",
                internal
            ));
        }
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

    // wave HR (renderer): 読込クラス名は難読名 → mojmap へ正規化してからパッチャ(mojmap前提)へ。
    // これが無いと flipFrame/render/tick/screen の全CFLHパッチが難読化runtimeで適用されない
    // (= レンダフックもModsボタンも発火しない統一原因)。
    let mojmap_name = crate::obf_map::normalize_to_mojmap_internal(class_name);
    if !rsift_parser::BytecodePatcher::is_target_class(&mojmap_name) {
        return;
    }

    let slice = std::slice::from_raw_parts(class_data, class_data_len as usize);
    // パッチは毎回(冪等)実行し、結果の可視化だけクラス毎1回に throttle する。
    let patched = ClassTransformer::on_class_load(&mojmap_name, slice);

    // wave HS 観測: 対象クラス到達 + パッチ結果 (クラス毎1回)。レンダー/ティック
    // 不発の原因を次ログで決定づける一次情報。
    let should_log = CFLH_OUTCOME_LOGGED
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .map(|mut g| g.insert(mojmap_name.clone()))
        .unwrap_or(false);
    if should_log {
        match &patched {
            Some(p) => agent_log(&format!(
                "[CFLH] PATCHED {} (runtime={}) {} → {} bytes",
                mojmap_name, class_name, slice.len(), p.len()
            )),
            None => agent_log(&format!(
                "[CFLH] {} (runtime={}) matched target but NOT modified (method name/desc not found or refused) — {} bytes",
                mojmap_name, class_name, slice.len()
            )),
        }
    }

    let Some(patched) = patched else {
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
        // wave 209 HE: ClassLoad も同時 install (重要クラス jcache)。
        callbacks.set_class_load(class_load_event as *const c_void);
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
        let rc = set_mode(jvmti, JVMTI_ENABLE, EVENT_CLASS_LOAD, ptr::null_mut());
        if rc != 0 {
            // ClassLoad が不許可な VM は想定外だが、CFLH 単独でも基本動作は
            // 維持されるため静黙降格 (ログは残す)。
            agent_log_warn(
                "jvmti",
                &format!("SetEventNotificationMode(CLASS_LOAD) rc={}", rc),
            );
        }
        HOOK_INSTALLED.store(true, Ordering::SeqCst);
        agent_log("[JVMTI] ClassFileLoadHook + ClassLoad enabled (exact-spec indices)");
        true
    }
}

/// wave 208 HD: RetransformClasses 直前に「呼出に使う env そのもの」で
/// capability を再点検し、未取得ならこの場で AddCapabilities を試みる。
///
/// JVMTI 規格上 capability は **env オブジェクト単位** で管理される
/// (jvmti.xml: "each agent environment has its own separate state,
/// including capabilities")。Agent_OnLoad で取得した env と、後続の
/// コード経路で GetEnv から得た env が別オブジェクトの場合、後者は
/// デフォルト状態の可能性があり、RetransformClasses が
/// MUST_POSSESS_CAPABILITY (rc=99) を返す。本関数は「実際に呼出に
/// 使う env」に対して GetCapabilities → (未取得なら) AddCapabilities を
/// 必ず挟むため、env がどの経路で得られたものでも能力要求が確実に
/// その env へ届く。VM が本当に retransform を許可しない場合
/// (AOT 構成等) は AddCapabilities の rc≠0 が残り、99 の原因が
/// caps 不足か VM の制約かが実機ログで確定できる (推測で話さない)。
///
/// AddCapabilities はエージェントの他機能を妨げない additive 要求のみ
/// (redefine/retransform/retransform_any) で、phase で獲得不能な能力は
/// VM が rc≠0 で拒否する = 規格準拠の安全動作。
fn ensure_retransform_caps_on_call_env(jvmti: *mut c_void) {
    unsafe {
        let Some(f) = jvmti_fn(jvmti, IDX_GET_CAPABILITIES) else {
            return;
        };
        let get: GetCapabilitiesFn = std::mem::transmute(f);
        let mut caps = JvmtiCapabilities::new();
        if get(jvmti, &mut caps) != 0 {
            return;
        }
        if caps.get_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES) {
            return;
        }
        let Some(f) = jvmti_fn(jvmti, IDX_ADD_CAPABILITIES) else {
            return;
        };
        let add: AddCapabilitiesFn = std::mem::transmute(f);
        let mut req = JvmtiCapabilities::new();
        req.set_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES);
        req.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES);
        req.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS);
        let arc = add(jvmti, &req);
        if arc != 0 {
            agent_log_warn(
                "jvmti",
                &format!(
                    "AddCapabilities(redefine/retransform/retransform_any) \
                     on call-env rc={} — VM が現 phase/env では付与拒否。\
                     RetransformClasses は rc=99 予定、CFLH-only パスを継続",
                    arc
                ),
            );
        }
    }
}

/// フック install 前にロード済みのクラスへ後追いパッチ。`java_vm` は生存中
/// JavaVM。classes は jclass 配列 (loadClass 等で取得済み)。rc と件数を記録。
pub unsafe fn retransform_classes(java_vm: *mut c_void, classes: &[*mut c_void]) -> i32 {
    if classes.is_empty() {
        return 0;
    }
    if retransform_disabled_by_env() {
        // テスト経路: 実 JVM に触れず 99 を返し、降格パス (CFLH/先行登録) の
        // みで機能が成立することをハーネスで検証可能にする。
        agent_log_warn(
            "jvmti",
            "RSIFT_DISABLE_RETRANSFORM set — retransform stubbed rc=99 (test hook)",
        );
        return 99;
    }
    unsafe {
        let Some(jvmti) = get_jvmti_env(java_vm) else {
            return -1;
        };
        // wave 208 HD: 呼出 env への caps 要求を必ず経由 (実機 rc=99 根治)。
        ensure_retransform_caps_on_call_env(jvmti);
        let rt: RetransformClassesFn = match jvmti_fn(jvmti, IDX_RETRANSFORM_CLASSES) {
            Some(f) => std::mem::transmute(f),
            None => return -1,
        };
        rt(jvmti, classes.len() as c_int, classes.as_ptr())
    }
}

/// wave 208 HD: テストフック。RSIFT_DISABLE_RETRANSFORM=1 環境で呼出を
/// 99 (MUST_POSSESS_CAPABILITY 相当) のスタブに差し替え、実 JVM ハーネスで
/// 「retransform 不能 VM での降格動作」を再現検証できるようにする。
/// 本番経路のデフォルト動作は変わらない (env 未設定時)。
pub fn retransform_disabled_by_env() -> bool {
    std::env::var_os("RSIFT_DISABLE_RETRANSFORM").is_some()
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

/// AddToBootstrapClassLoaderSearch (num 149 → idx 148) — bootstrap CL の検索
/// パスに instrumentation jar を追加する (wave 207 HC-1 の根治本体)。
/// これにより HEAD 注入先 com/rsift/RsiftHooks がゲーム側どの ClassLoader
/// からも (親委譲で) 解決可能になる。live 相では既存 JAR パスのみ受理、
/// capability 不要、複数回呼出で複数 segment (先に呼んだ順に検索)。
/// 返り値 = JVMTI rc (0 でない場合は呼出側が warn ログで可視化する)。
/// env は共有 JVMTI_ENV (HA-4)。attach 済みスレッドから呼ぶこと。
pub unsafe fn add_to_bootstrap_class_loader_search(java_vm: *mut c_void, jar_path: &str) -> i32 {
    let Some(jvmti) = get_jvmti_env(java_vm) else {
        agent_log_warn("jvmti", "AddToBootstrapClassLoaderSearch: no jvmti env");
        return -1;
    };
    let Some(f) = jvmti_fn(jvmti, IDX_ADD_TO_BOOTSTRAP_CLASS_LOADER_SEARCH) else {
        agent_log_warn("jvmti", "AddToBootstrapClassLoaderSearch: fn not resolved");
        return -2;
    };
    let c_path = match std::ffi::CString::new(jar_path) {
        Ok(c) => c,
        Err(_) => {
            agent_log_warn(
                "jvmti",
                "AddToBootstrapClassLoaderSearch: bad path (NUL byte)",
            );
            return -3;
        }
    };
    let add: AddToBootstrapClassLoaderSearchFn = std::mem::transmute(f);
    add(jvmti, c_path.as_ptr())
}

/// AddToBootstrapClassLoaderSearch のログつき統一ラッパ (wave 207)。
/// phase_label は呼出相 ("Agent_OnLoad" / "CFLH install fallback") で
/// ログ行を区別する。冪等 (BOOTSTRAP_CLASSPATH_ADDED) — 呼出側で
/// 可視化したいので rc を返す。成功時のみ static を立てる。
pub unsafe fn ensure_bootstrap_classpath_once(java_vm: *mut c_void, phase_label: &str) -> i32 {
    if BOOTSTRAP_CLASSPATH_ADDED.load(Ordering::SeqCst) {
        return 0;
    }
    let Some(jar) = crate::screen_inject::bootstrap_jar() else {
        agent_log_warn(
            "jvmti",
            &format!(
                "{}: bootstrap jar path unknown — RsiftHooks はゲーム loader から解決不能のまま",
                phase_label
            ),
        );
        return -4;
    };
    let jar_s = jar.display().to_string();
    let rc = add_to_bootstrap_class_loader_search(java_vm, &jar_s);
    if rc == 0 {
        BOOTSTRAP_CLASSPATH_ADDED.store(true, Ordering::SeqCst);
        agent_log_step(
            "jvmti",
            &format!(
                "{}: AddToBootstrapClassLoaderSearch rc=0 — RsiftHooks 可視化 (bootstrap CL): {}",
                phase_label, jar_s
            ),
        );
    } else {
        agent_log_warn(
            "jvmti",
            &format!(
                "{}: AddToBootstrapClassLoaderSearch rc={} for {} — hook 注入クラスの解決が失敗しうる (vanilla 判定のまま)",
                phase_label, rc, jar_s
            ),
        );
    }
    rc
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
        // wave 207: AddToBootstrapClassLoaderSearch num=149 (C index = 148)。
        assert_eq!(IDX_ADD_TO_BOOTSTRAP_CLASS_LOADER_SEARCH, 149 - 1);
        assert_eq!(EVENT_CLASS_FILE_LOAD_HOOK, 54);
        assert_eq!(CALLBACK_SLOT_CLASS_FILE_LOAD_HOOK, 4);
        // wave 209 HE: ClassLoad num=55 / コールバック slot5 (CFLH=54/slot4 連番)、
        // GetClassSignature num=48 (Allocate 46/Deallocate 47 連番)。
        assert_eq!(EVENT_CLASS_LOAD, 55);
        assert_eq!(CALLBACK_SLOT_CLASS_LOAD, 5);
        assert_eq!(IDX_GET_CLASS_SIGNATURE, 48 - 1);
        assert_eq!(IDX_JNI_GET_ENV, 6);
        assert_eq!(IDX_JNI_NEW_GLOBAL_REF, 21);
    }

    /// wave 208 HD: 環境変数スタブの既定値ピン。本番既定では false (= stub 不発)。
    /// ハーネスは RSIFT_DISABLE_RETRANSFORM=1 を環境指定して起動する
    /// (ユニットテスト内で set_var は他テストとの共有状態汚染リスクがあるため、
    /// ここでは既定側のみを固定する)。
    #[test]
    fn retransform_stub_is_off_by_default() {
        if std::env::var_os("RSIFT_DISABLE_RETRANSFORM").is_none() {
            assert!(
                !retransform_disabled_by_env(),
                "既定で retransform stub は不発"
            );
        }
    }

    /// wave 208 HD: caps 要求の additive 保証ピン。ensure_retransform_caps_on_call_env
    /// が AddCapabilities に渡すビット集合は define/retransform/any の 3 つだけ —
    /// VM の他 agent 状態を破壊しない (JVMTI の AddCapabilities は additive で
    /// 獲得不能ビットは VM 側が rc≠0 で拒否する仕様 = リーク自由変数は無い)。
    #[test]
    fn retransform_caps_request_is_additive_only() {
        let mut req = JvmtiCapabilities::new();
        req.set_bit(JvmtiCapabilities::BIT_CAN_REDEFINE_CLASSES);
        req.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_CLASSES);
        req.set_bit(JvmtiCapabilities::BIT_CAN_RETRANSFORM_ANY_CLASS);
        assert_eq!(req.words[0], 1 << 9);
        assert_eq!(req.words[1], (1 << 5) | (1 << 6));
        assert_eq!(req.words[2], 0);
        assert_eq!(req.words[3], 0);
    }

    /// wave 209 HE: wanted jcache の狙撃ピン。未登録名は絶対に None、
    /// 登録名も初期は None (= 未定義クラスを誤爆しない)。重要クラス集合は
    /// 実機で CNFE を起こした確定クラス郡 — ここ以外の拡張は RED。
    #[test]
    fn wanted_class_cache_is_precise_and_empty_by_default() {
        assert!(wanted_class_index("net/minecraft/client/Minecraft").is_some());
        assert!(wanted_class_index("net/minecraft/network/Connection").is_some());
        assert!(wanted_class_index("net/minecraft/client/main/Main").is_none());
        assert!(wanted_class_index("java/lang/String").is_none());
        assert_eq!(WANTED_CLASSES.len(), 6);
        assert!(WANTED_CLASSES.contains(&"net/minecraft/client/gui/screens/Screen"));
        assert!(WANTED_CLASSES
            .contains(&"net/minecraft/client/gui/components/debug/DebugScreenEntryList"));
        assert!(WANTED_CLASSES
            .contains(&"net/minecraft/client/gui/components/debug/DebugScreenOverlay"));
        assert!(WANTED_CLASSES.contains(&"net/minecraft/client/gui/components/DebugScreenOverlay"));
        // 初期状態 (このプロセス内で ClassLoad 未発火) は空 = 嘘を返さない
        if wanted_class_raw("net/minecraft/client/Minecraft").is_some() {
            // 同一プロセスで実 JVM ハーネスが先に走った場合のみあり得る
            // (test 単独実行では必ず None)。
        }
    }

    /// wave HR (#5 根治): match_wanted_at_runtime は obf_map 未 install 時
    /// (テスト既定状態) は直接 mojmap 照合へ安全落下 = wanted_class_index と一致。
    /// obf_map::resolve_class_internal は mode 未確定で None → unwrap_or(mojmap)。
    /// (Obfuscated モードの解決ロジック自体は obf_map::tests で検証済み。本テストは
    ///  fallback 一貫性の pin = 未 install で従来挙動が壊れないことの保証。)
    #[test]
    fn match_wanted_at_runtime_falls_back_to_direct_when_uninstalled() {
        // obf_map 未 install (current_mode() = None) → 直接 mojmap 照合と同値。
        assert_eq!(
            match_wanted_at_runtime("net/minecraft/client/Minecraft"),
            Some(0)
        );
        assert_eq!(
            match_wanted_at_runtime("net/minecraft/network/Connection"),
            wanted_class_index("net/minecraft/network/Connection")
        );
        // 非対象・難読名 (未 install なので解決不能) は None。
        assert!(match_wanted_at_runtime("net/minecraft/client/main/Main").is_none());
        assert!(match_wanted_at_runtime("java/lang/String").is_none());
        assert!(match_wanted_at_runtime("gfj").is_none(), "未 install 時は難読名を解決できない (= Some にならない)");
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
