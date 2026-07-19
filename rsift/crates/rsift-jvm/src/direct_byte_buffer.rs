
//! Long-lived DirectByteBuffer共有 - Rust確保メモリをJavaに渡しっぱなしでゼロコピー
//! NewDirectByteBufferでJavaに渡し、Java側からはオフヒープ配列として見える

use jni::JNIEnv;
use jni::objects::{JByteBuffer, JClass};
use std::alloc::{alloc, dealloc, Layout};

pub struct SharedDirectBuffer {
    ptr: *mut u8,
    cap: usize,
    layout: Layout,
}

impl SharedDirectBuffer {
    pub fn new(cap: usize) -> Self {
        let layout = Layout::from_size_align(cap, 64).unwrap();
        let ptr = unsafe { alloc(layout) };
        Self { ptr, cap, layout }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.cap) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.cap) }
    }

    /// JNI NewDirectByteBufferでJava側に公開（lifetimeはRust側が管理）
    pub fn to_java<'a>(&self, env: &mut JNIEnv<'a>) -> Result<JByteBuffer<'a>, String> {
        let buf = unsafe { env.new_direct_byte_buffer(self.ptr, self.cap).map_err(|e| e.to_string())? };
        Ok(buf)
    }
}

impl Drop for SharedDirectBuffer {
    fn drop(&mut self) { unsafe { dealloc(self.ptr, self.layout) } }
}
