
//! C ABI VTableによる1回だけのバインディング - 起動時にテーブルをJavaに渡しGetMethodID検索0

#[repr(C)]
pub struct JavaBridgeVTable {
    pub version: u32,
    pub update_chunk: extern "C" fn(i32, i32, *const u8, usize) -> i32,
    pub set_camera: extern "C" fn(f32, f32, f32, f32, f32),
    pub log: extern "C" fn(*const u8, usize),
}

impl JavaBridgeVTable {
    pub fn new() -> Self {
        Self {
            version: 2,
            update_chunk: dummy_update_chunk,
            set_camera: dummy_set_camera,
            log: dummy_log,
        }
    }

    pub fn as_ptr(&self) -> *const Self { self as *const _ }
}

extern "C" fn dummy_update_chunk(_x: i32, _z: i32, _data: *const u8, _len: usize) -> i32 { 0 }
extern "C" fn dummy_set_camera(_x: f32, _y: f32, _z: f32, _yaw: f32, _pitch: f32) {}
extern "C" fn dummy_log(_ptr: *const u8, _len: usize) {}

// Java側でこのテーブルを受け取り、以後は関数ポインタ直呼び出し
pub fn install_vtable() -> JavaBridgeVTable {
    JavaBridgeVTable::new()
}
