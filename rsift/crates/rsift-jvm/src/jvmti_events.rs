//! Minimal JVMTI ClassFileLoadHook — rewrites target classes in native code (no Java JNI during load).

use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_log::agent_log;
use crate::jvmti_hook::ClassTransformer;

const JVMTI_VERSION_1_2: c_int = 0x30010002;
const JVMTI_ENABLE: c_int = 1;
const JVMTI_EVENT_CLASS_FILE_LOAD_HOOK: c_int = 54;

static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct JvmtiCapabilities {
    bits: [u8; 16],
}

impl JvmtiCapabilities {
    fn new() -> Self {
        Self { bits: [0; 16] }
    }

    fn set_can_generate_all_class_hook_events(&mut self) {
        // bit 0 of byte 0 in classic layout varies; set several capability bits used by HotSpot
        self.bits[0] |= 0x01; // can_tag_objects-ish — we also set class hook via AddCapabilities broadly
        self.bits[1] |= 0x40; // can_generate_all_class_hook_events often around here on JDK17+
        self.bits[2] |= 0x01;
        self.bits[3] |= 0x80;
    }
}

/// Subset of jvmtiInterface — only the slots we call (indices match JVM TI 1.x HotSpot layout).
#[repr(C)]
struct JvmtiInterface {
    // We resolve functions dynamically from the env vtable by scanning isn't reliable.
    // Instead store function pointers we look up once via GetEnv + known offsets.
    _opaque: [*mut c_void; 1],
}

type AllocateFn = unsafe extern "system" fn(*mut c_void, i64, *mut *mut c_uchar) -> c_int;
type AddCapabilitiesFn = unsafe extern "system" fn(*mut c_void, *const JvmtiCapabilities) -> c_int;
type SetEventCallbacksFn = unsafe extern "system" fn(*mut c_void, *const JvmtiEventCallbacks, c_int) -> c_int;
type SetEventNotificationModeFn =
    unsafe extern "system" fn(*mut c_void, c_int, c_int, *mut c_void) -> c_int;
type DeallocateFn = unsafe extern "system" fn(*mut c_void, *mut c_uchar) -> c_int;

#[repr(C)]
struct JvmtiEventCallbacks {
    data: [u8; 1024],
}

impl JvmtiEventCallbacks {
    fn zeroed() -> Self {
        Self { data: [0u8; 1024] }
    }

    /// ClassFileLoadHook is event 54; pointer stored at index 54 in the callbacks struct
    /// (each slot is a function pointer). Layout: array of function pointers.
    unsafe fn set_class_file_load_hook(&mut self, cb: *const c_void) {
        let slot = 54usize;
        let ptrs = self.data.as_mut_ptr() as *mut *const c_void;
        *ptrs.add(slot) = cb;
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
    _jni_env: *mut c_void,
    _class_being_redefined: *mut c_void,
    _loader: *mut c_void,
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

    // Allocate via JVMTI Allocate (function index 46 in JVMTI 1.2 HotSpot)
    let allocate: AllocateFn = match jvmti_fn(jvmti_env, 46) {
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

unsafe fn jvmti_fn(env: *mut c_void, index: usize) -> Option<*mut c_void> {
    if env.is_null() {
        return None;
    }
    // jvmtiEnv* -> functions table*
    let functions = *(env as *mut *mut *mut c_void);
    if functions.is_null() {
        return None;
    }
    Some(*functions.add(index))
}

/// Install ClassFileLoadHook using the live JavaVM pointer from deferred attach.
pub fn install_class_file_load_hook(java_vm: *mut c_void) -> bool {
    if HOOK_INSTALLED.load(Ordering::SeqCst) {
        return true;
    }
    if java_vm.is_null() {
        return false;
    }

    unsafe {
        // JNIInvokeInterface GetEnv is index 6
        let invoke = *(java_vm as *mut *mut *mut c_void);
        if invoke.is_null() {
            return false;
        }
        type GetEnvFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int;
        let get_env: GetEnvFn = std::mem::transmute(*invoke.add(6));
        let mut jvmti: *mut c_void = ptr::null_mut();
        let rc = get_env(java_vm, &mut jvmti, JVMTI_VERSION_1_2);
        if rc != 0 || jvmti.is_null() {
            // try version 9
            let rc9 = get_env(java_vm, &mut jvmti, 0x30000000 | 9);
            if rc9 != 0 || jvmti.is_null() {
                agent_log(&format!("[JVMTI] GetEnv failed rc={}", rc));
                return false;
            }
        }

        let mut caps = JvmtiCapabilities::new();
        caps.set_can_generate_all_class_hook_events();
        // AddCapabilities index 142 in JVMTI 1.2 — also try 23 (common for AddCapabilities)
        for idx in [23usize, 142, 22] {
            if let Some(f) = jvmti_fn(jvmti, idx) {
                let add: AddCapabilitiesFn = std::mem::transmute(f);
                let _ = add(jvmti, &caps);
            }
        }

        let mut callbacks = JvmtiEventCallbacks::zeroed();
        callbacks.set_class_file_load_hook(class_file_load_hook as *const c_void);

        // SetEventCallbacks ~ index 120
        let mut ok = false;
        for idx in [120usize, 122, 86] {
            if let Some(f) = jvmti_fn(jvmti, idx) {
                let set_cb: SetEventCallbacksFn = std::mem::transmute(f);
                let size = std::mem::size_of::<JvmtiEventCallbacks>() as c_int;
                if set_cb(jvmti, &callbacks, size) == 0 {
                    ok = true;
                    break;
                }
            }
        }
        if !ok {
            agent_log("[JVMTI] SetEventCallbacks failed");
            return false;
        }

        // SetEventNotificationMode ~ index 58
        for idx in [58usize, 75] {
            if let Some(f) = jvmti_fn(jvmti, idx) {
                let set_mode: SetEventNotificationModeFn = std::mem::transmute(f);
                if set_mode(jvmti, JVMTI_ENABLE, JVMTI_EVENT_CLASS_FILE_LOAD_HOOK, ptr::null_mut()) == 0
                {
                    HOOK_INSTALLED.store(true, Ordering::SeqCst);
                    agent_log("[JVMTI] ClassFileLoadHook enabled");
                    return true;
                }
            }
        }
        agent_log("[JVMTI] SetEventNotificationMode failed");
        false
    }
}

/// Fallback: transform via Rust when Java asks (safe after bootstrap).
pub fn transform_bytes(class_name: &str, data: &[u8]) -> Option<Vec<u8>> {
    ClassTransformer::on_class_load(class_name, data)
}
