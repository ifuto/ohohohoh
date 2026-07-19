//! Phase 1 — Krypton-style network: VarInt fast path, packet buffer pool, auto compression threshold.

use parking_lot::Mutex;
use std::collections::VecDeque;

/// Fast VarInt decode — branch-light for common 1-byte case.
#[inline]
pub fn read_varint(buf: &[u8]) -> Option<(i32, usize)> {
    if buf.is_empty() {
        return None;
    }
    let b0 = buf[0];
    if b0 & 0x80 == 0 {
        return Some((b0 as i32, 1));
    }
    let mut value = 0i32;
    let mut pos = 0usize;
    let mut shift = 0u32;
    loop {
        if pos >= buf.len() || shift >= 35 {
            return None;
        }
        let b = buf[pos];
        pos += 1;
        value |= ((b & 0x7f) as i32) << shift;
        if b & 0x80 == 0 {
            return Some((value, pos));
        }
        shift += 7;
    }
}

#[inline]
pub fn write_varint(mut value: i32, out: &mut Vec<u8>) {
    loop {
        if (value & !0x7f) == 0 {
            out.push(value as u8);
            return;
        }
        out.push(((value & 0x7f) | 0x80) as u8);
        value = ((value as u32) >> 7) as i32;
    }
}

pub struct PacketBufferPool {
    free: Mutex<VecDeque<Vec<u8>>>,
    capacity: usize,
    buf_cap: usize,
}

impl PacketBufferPool {
    pub fn new(capacity: usize, buf_cap: usize) -> Self {
        let mut free = VecDeque::with_capacity(capacity);
        for _ in 0..capacity.min(32) {
            free.push_back(Vec::with_capacity(buf_cap));
        }
        Self {
            free: Mutex::new(free),
            capacity,
            buf_cap,
        }
    }

    pub fn acquire(&self) -> Vec<u8> {
        let mut g = self.free.lock();
        g.pop_front().unwrap_or_else(|| Vec::with_capacity(self.buf_cap))
    }

    pub fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        if buf.capacity() > self.buf_cap * 4 {
            buf.shrink_to(self.buf_cap);
        }
        let mut g = self.free.lock();
        if g.len() < self.capacity {
            g.push_back(buf);
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressionPolicy {
    pub threshold: usize,
    pub min_threshold: usize,
    pub max_threshold: usize,
    ema_size: f32,
}

impl CompressionPolicy {
    pub fn new(threshold: usize) -> Self {
        Self {
            threshold,
            min_threshold: 64,
            max_threshold: 8192,
            ema_size: threshold as f32,
        }
    }

    /// Auto-adjust: larger average packets → higher threshold (avoid tiny compress).
    pub fn observe_packet_size(&mut self, size: usize) {
        self.ema_size = self.ema_size * 0.95 + size as f32 * 0.05;
        let target = (self.ema_size * 0.5) as usize;
        self.threshold = target.clamp(self.min_threshold, self.max_threshold);
    }

    pub fn should_compress(&self, size: usize) -> bool {
        size >= self.threshold
    }
}

/// Optional QUIC planner for Rsift-dedicated links (not vanilla MC protocol).
/// Caps concurrent streams; never opens unbounded connections.
#[derive(Debug, Clone)]
pub struct QuicChannelPlanner {
    pub max_streams: u32,
    pub max_idle_ms: u64,
    pub enable: bool,
    active_streams: u32,
}

impl QuicChannelPlanner {
    pub fn new(max_streams: u32) -> Self {
        Self {
            max_streams: max_streams.clamp(1, 256),
            max_idle_ms: 30_000,
            enable: false,
            active_streams: 0,
        }
    }

    /// Enable only for Rsift↔Rsift dedicated transport — never for vanilla clients.
    pub fn enable_rsift_only(&mut self) {
        self.enable = true;
    }

    pub fn try_open_stream(&mut self) -> bool {
        if !self.enable || self.active_streams >= self.max_streams {
            return false;
        }
        self.active_streams += 1;
        true
    }

    pub fn close_stream(&mut self) {
        self.active_streams = self.active_streams.saturating_sub(1);
    }

    pub fn hint(&self) -> TransportHint {
        if self.enable {
            TransportHint::QuicExperimental
        } else {
            TransportHint::TcpLegacy
        }
    }
}

/// Optional QUIC marker — Rsift-dedicated channel may enable later; TCP path remains default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportHint {
    TcpLegacy,
    QuicExperimental,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [0, 1, 127, 128, 255, 30000, -1] {
            let mut buf = Vec::new();
            write_varint(v, &mut buf);
            let (got, n) = read_varint(&buf).unwrap();
            assert_eq!(got, v);
            assert_eq!(n, buf.len());
        }
    }

    #[test]
    fn pool_reuses() {
        let pool = PacketBufferPool::new(4, 256);
        let a = pool.acquire();
        pool.release(a);
        let b = pool.acquire();
        assert!(b.capacity() >= 256);
    }
}
