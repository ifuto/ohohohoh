//! Zero-copy packet capture types

use bytemuck::{Pod, Zeroable};
use rsift_api::packet::DirectBufferSlice;
use tracing::trace;

/// Packet direction
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDirection {
    Incoming = 0,
    Outgoing = 1,
}

/// Header for each recorded packet in raw stream
#[derive(Debug, Clone, Copy)]
pub struct PacketRecordHeader {
    pub timestamp_us: u64,
    pub direction: u8,
    pub packet_id: u32,
    pub payload_len: u32,
    pub view_mode: u8,
}

/// F5 view mode change event
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ViewModeEvent {
    pub timestamp_us: u64,
    pub mode: u8,
    pub _pad: [u8; 7],
}

/// Player camera state snapshot (first-person centric)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CameraSnapshot {
    pub timestamp_us: u64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub view_mode: u8,
    pub _pad: [u8; 3],
}

/// Zero-copy capture from JNI DirectBuffer
pub struct ZeroCopyCapture;

impl ZeroCopyCapture {
    /// Capture packet without copying JVM heap — reads direct buffer via bytemuck
    pub unsafe fn capture_from_jni(
        timestamp_us: u64,
        direction: PacketDirection,
        packet_id: u32,
        ptr: i64,
        len: i32,
        view_mode: u8,
    ) -> Result<CapturedPacket, &'static str> {
        let slice = DirectBufferSlice::from_raw_jni(ptr, len)?;
        let payload = slice.as_slice().to_vec(); // ring buffer owns copy once; JNI buffer not retained
        trace!(
            "[RsReplay] zero-copy capture pkt={} dir={:?} len={}",
            packet_id, direction, len
        );
        Ok(CapturedPacket {
            header: PacketRecordHeader {
                timestamp_us,
                direction: direction as u8,
                packet_id,
                payload_len: payload.len() as u32,
                view_mode,
            },
            payload,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CapturedPacket {
    pub header: PacketRecordHeader,
    pub payload: Vec<u8>,
}
