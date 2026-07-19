//! # Zero-Copy Packet Layer using `bytemuck`
//!
//! Minecraft (Netty) の `ByteBuffer.allocateDirect()` (オフヒープメモリ) から
//! 通知されたアドレス (`long`) とサイズ (`int`) を用いて、
//! 1ビットのメモリコピーも行わずに直接Rustの構造体にキャストして解析・処理します。

use bytemuck::{Pod, Zeroable};
use std::slice;
use tracing::{error, trace};

#[repr(C)]
#[derive(Debug, Clone, Copy, Zeroable)]
pub struct PacketHeader {
    pub packet_id: u32,
    pub payload_length: u32,
    pub sequence: u64,
}

unsafe impl Pod for PacketHeader {}

#[repr(C)]
#[derive(Debug, Clone, Copy, Zeroable)]
pub struct PlayerPositionPacket {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: u32,
    pub _pad: u32,
}

unsafe impl Pod for PlayerPositionPacket {}

#[repr(C)]
#[derive(Debug, Clone, Copy, Zeroable)]
pub struct SpawnEntityPacket {
    pub entity_id: i32,
    pub entity_type: u32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub velocity_z: f32,
    pub _pad: u32,
}

unsafe impl Pod for SpawnEntityPacket {}

#[derive(Debug)]
pub struct DirectBufferSlice<'a> {
    pub ptr: *const u8,
    pub len: usize,
    slice: &'a [u8],
}

impl<'a> DirectBufferSlice<'a> {
    pub unsafe fn from_raw_jni(ptr: i64, len: i32) -> Result<Self, &'static str> {
        if ptr == 0 {
            return Err("Null pointer passed from JNI Direct ByteBuffer");
        }
        if len < 0 {
            return Err("Negative length passed from JNI");
        }
        let raw_ptr = ptr as *const u8;
        let size = len as usize;
        let slice = slice::from_raw_parts(raw_ptr, size);
        Ok(Self {
            ptr: raw_ptr,
            len: size,
            slice,
        })
    }

    pub fn as_pod<T: Pod>(&self) -> Result<&'a T, &'static str> {
        if self.len < std::mem::size_of::<T>() {
            error!(
                "Direct buffer size ({}) is smaller than required Pod size ({})",
                self.len,
                std::mem::size_of::<T>()
            );
            return Err("Buffer size too small for target Pod structure");
        }
        let target_bytes = &self.slice[..std::mem::size_of::<T>()];
        match bytemuck::try_from_bytes(target_bytes) {
            Ok(pod) => {
                trace!("Zero-copy cast to Pod succeeded (size: {})", std::mem::size_of::<T>());
                Ok(pod)
            }
            Err(_) => Err("bytemuck cast failed due to alignment or layout mismatch"),
        }
    }

    pub fn as_slice(&self) -> &'a [u8] {
        self.slice
    }
}

pub struct DirectBufferSliceMut<'a> {
    pub ptr: *mut u8,
    pub len: usize,
    slice: &'a mut [u8],
}

impl<'a> DirectBufferSliceMut<'a> {
    pub unsafe fn from_raw_jni_mut(ptr: i64, len: i32) -> Result<Self, &'static str> {
        if ptr == 0 {
            return Err("Null pointer passed from JNI Direct ByteBuffer");
        }
        if len < 0 {
            return Err("Negative length passed from JNI");
        }
        let raw_ptr = ptr as *mut u8;
        let size = len as usize;
        let slice = slice::from_raw_parts_mut(raw_ptr, size);
        Ok(Self {
            ptr: raw_ptr,
            len: size,
            slice,
        })
    }

    pub fn as_pod_mut<T: Pod>(&'a mut self) -> Result<&'a mut T, &'static str> {
        if self.len < std::mem::size_of::<T>() {
            return Err("Buffer size too small for target Pod structure");
        }
        let target_bytes = &mut self.slice[..std::mem::size_of::<T>()];
        match bytemuck::try_from_bytes_mut(target_bytes) {
            Ok(pod) => Ok(pod),
            Err(_) => Err("bytemuck mutable cast failed"),
        }
    }
}
