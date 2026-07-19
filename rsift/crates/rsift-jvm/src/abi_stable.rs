
//! ABI安定Cインターフェース + Semver Feature Negotiation
//! #[repr(C)]なAPIでRust更新でもModが壊れにくい

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RsiftApiVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[repr(C)]
pub struct RsiftModApi {
    pub version: RsiftApiVersion,
    pub init: extern "C" fn(*mut std::os::raw::c_void) -> i32,
    pub tick: extern "C" fn(f32),
    pub shutdown: extern "C" fn(),
}

impl RsiftModApi {
    pub fn current() -> Self {
        Self {
            version: RsiftApiVersion { major: 1, minor: 21, patch: 11 },
            init: dummy_init,
            tick: dummy_tick,
            shutdown: dummy_shutdown,
        }
    }

    pub fn negotiate(&self, other: &RsiftApiVersion) -> bool {
        self.version.major == other.major && self.version.minor >= other.minor
    }
}

extern "C" fn dummy_init(_ctx: *mut std::os::raw::c_void) -> i32 { 0 }
extern "C" fn dummy_tick(_dt: f32) {}
extern "C" fn dummy_shutdown() {}
