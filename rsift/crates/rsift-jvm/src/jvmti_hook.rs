//! JVMTI Agent_OnLoad — minimal safe entry (no mod load / no JNI here).

use rsift_parser::BytecodePatcher;
use std::ffi::CStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{info, trace, warn};

use crate::agent_log::{agent_log_session_start, agent_log_step};

fn bootstrap_log(line: &str) {
    crate::agent_log::agent_log(line);
}

/// JVM `-agentpath:rsift_jvm.dll` entry. Must stay minimal to avoid 0xC0000005.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn Agent_OnLoad(vm: *mut c_void, options: *mut c_char, _reserved: *mut c_void) -> c_int {
    let opt_str = if options.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(options).to_string_lossy().into_owned() }
    };

    agent_log_session_start(&format!("agentpath opts={}", opt_str));
    agent_log_step("Agent_OnLoad", "entered");
    agent_log_step("Agent_OnLoad", &format!("options={:?}", opt_str));

    agent_log_step("Agent_OnLoad", "scheduling deferred init thread (mods load in background thread)");
    crate::agent_bridge::schedule_deferred_init(vm, &opt_str, true);
    agent_log_step("Agent_OnLoad", "complete — returning 0 to JVM");

    0
}

pub static TOTAL_CLASSES_SCANNED: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_CLASSES_PATCHED: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_TIME_NANOS: AtomicU64 = AtomicU64::new(0);

pub struct ClassTransformer;

impl ClassTransformer {
    pub fn on_class_load(class_name: &str, class_data: &[u8]) -> Option<Vec<u8>> {
        let start_time = Instant::now();
        TOTAL_CLASSES_SCANNED.fetch_add(1, Ordering::Relaxed);

        match BytecodePatcher::patch_if_needed(class_name, class_data) {
            Ok(result) => {
                let elapsed = start_time.elapsed().as_nanos() as u64;
                TOTAL_TIME_NANOS.fetch_add(elapsed, Ordering::Relaxed);

                if result.was_modified {
                    TOTAL_CLASSES_PATCHED.fetch_add(1, Ordering::Relaxed);
                    info!(
                        "Rsift-Parser patched [{}] in {} µs",
                        class_name,
                        elapsed as f64 / 1000.0,
                    );
                }
                if result.was_modified {
                    Some(result.new_bytecode)
                } else {
                    trace!("Class [{}] clean", class_name);
                    None
                }
            }
            Err(e) => {
                warn!("Rsift-Parser error on [{}]: {}", class_name, e);
                None
            }
        }
    }

    pub fn print_statistics() {
        let scanned = TOTAL_CLASSES_SCANNED.load(Ordering::Relaxed);
        let patched = TOTAL_CLASSES_PATCHED.load(Ordering::Relaxed);
        info!("Rsift-Parser: scanned={} patched={}", scanned, patched);
    }
}
