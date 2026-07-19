//! テクスチャストリーミング — VRAM 予算内で必要な mip/テクスチャだけを常駐させ、
//! LRU で退避し、ヒステリシスでスラッシングを防ぐ。
//!
//! 統合メモリ（Apple Silicon / 内蔵GPU・GPU共有）環境ではスパーステクスチャや
//! ストリーミングが特に有効。外付け帯域を節約しつつ必要な解像度を確保する。

use std::collections::HashMap;
use std::collections::VecDeque;

pub struct TextureStreamer {
    pub budget_bytes: u64,
    /// 0..1。予算の (1-hysteresis) までは許容し、余裕ができても即退避しない。
    pub hysteresis: f64,
    resident: HashMap<u32, u64>,
    lru: VecDeque<u32>,
    sizes: HashMap<u32, u64>,
}

impl TextureStreamer {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            hysteresis: 0.1,
            resident: HashMap::new(),
            lru: VecDeque::new(),
            sizes: HashMap::new(),
        }
    }

    pub fn set_size(&mut self, id: u32, bytes: u64) {
        self.sizes.insert(id, bytes);
    }

    fn used(&self) -> u64 {
        self.resident.values().sum()
    }

    /// テクスチャを要求（常駐化）。予算超過なら LRU から退避。
    pub fn request(&mut self, id: u32) {
        if let Some(&sz) = self.sizes.get(&id) {
            if !self.resident.contains_key(&id) {
                let limit = (self.budget_bytes as f64 * (1.0 - self.hysteresis)) as u64;
                let mut used = self.used();
                while used + sz > limit && !self.lru.is_empty() {
                    let victim = self.lru.pop_front().unwrap();
                    if let Some(vs) = self.resident.remove(&victim) {
                        used -= vs;
                    }
                }
                self.resident.insert(id, sz);
                self.lru.push_back(id);
            } else {
                self.lru.retain(|&x| x != id);
                self.lru.push_back(id);
            }
        }
    }

    pub fn resident_bytes(&self) -> u64 {
        self.used()
    }

    pub fn is_resident(&self, id: u32) -> bool {
        self.resident.contains_key(&id)
    }
}

pub struct MipStreaming;
impl MipStreaming {
    pub fn wgsl_source(&self) -> &'static str {
        MIP_STREAMING_WGSL
    }
}
pub const MIP_STREAMING_WGSL: &str = include_str!("../shaders/mip_streaming.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_within_budget() {
        let mut s = TextureStreamer::new(1000);
        for i in 0..10u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        assert!(s.resident_bytes() <= 1000, "over budget: {}", s.resident_bytes());
        assert!(s.is_resident(9), "most-recent should stay");
        assert!(!s.is_resident(0), "oldest should be evicted");
    }

    #[test]
    fn rerequest_keeps_resident() {
        let mut s = TextureStreamer::new(1000);
        s.set_size(1, 200);
        s.request(1);
        s.set_size(2, 200);
        s.request(2);
        s.request(1); // touch 1
        assert!(s.is_resident(1));
    }

    #[test]
    fn hysteresis_prevents_immediate_thrash() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = 0.5; // allow up to 500 extra
        for i in 0..6u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        assert!(s.resident_bytes() <= 1000);
    }
}
