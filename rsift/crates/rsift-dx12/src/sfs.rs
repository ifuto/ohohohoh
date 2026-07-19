//! Phase 3 — Sampler Feedback Streaming (SFS) + feedback resolve.
//!
//! Intel / Microsoft SFS: resolve MinMip feedback → CPU residency → stream tiles.
//! Low-spec: hard cap tiles/frame, prefer coarse mips, no-op when feedback empty.

use crate::device::Dx12Device;
use crate::error::{Dx12Error, Dx12Result};
use crate::tiled_resources::TiledAtlas;
use crate::win::{
    DXGI_FORMAT_SAMPLER_FEEDBACK_MIN_MIP_OPAQUE, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use tracing::{debug, info};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

/// MinMip unused sentinel after `DECODE_SAMPLER_FEEDBACK` (DirectX Specs).
pub const SFS_UNUSED_MIP: u8 = 0xFF;

/// Default max tiles streamed per frame on weak hardware (VRAM + I/O budget).
pub const LOW_SPEC_TILES_PER_FRAME: u32 = 8;
pub const HIGH_SPEC_TILES_PER_FRAME: u32 = 32;

/// Pure CPU residency / feedback decode — independent of D3D12 COM objects.
#[derive(Debug, Clone)]
pub struct SfsResidencyTracker {
    pub feedback_width: u32,
    pub feedback_height: u32,
    pub resident: Vec<u64>,
    pub last_feedback: Vec<u8>,
    pub max_tiles_per_frame: u32,
}

impl SfsResidencyTracker {
    pub fn new(feedback_width: u32, feedback_height: u32, max_tiles_per_frame: u32) -> Self {
        let fw = feedback_width.max(1);
        let fh = feedback_height.max(1);
        let tile_estimate = ((fw * fh) / 4).max(64);
        Self {
            feedback_width: fw,
            feedback_height: fh,
            resident: Self::tile_bits(tile_estimate),
            last_feedback: vec![SFS_UNUSED_MIP; (fw * fh) as usize],
            max_tiles_per_frame: max_tiles_per_frame.max(1),
        }
    }

    fn tile_bits(tile_count: u32) -> Vec<u64> {
        let words = ((tile_count as usize) + 63) / 64;
        vec![0u64; words.max(1)]
    }

    #[inline]
    pub fn is_resident(&self, tile: u32) -> bool {
        let i = tile as usize;
        let word = i / 64;
        let bit = i % 64;
        self.resident
            .get(word)
            .map(|w| (w >> bit) & 1 != 0)
            .unwrap_or(false)
    }

    pub fn mark_resident(&mut self, tile: u32) {
        let i = tile as usize;
        let word = i / 64;
        let bit = i % 64;
        if word >= self.resident.len() {
            self.resident.resize(word + 1, 0);
        }
        self.resident[word] |= 1u64 << bit;
    }

    /// Ingest CPU-mapped MinMip resolve bytes (R8_UINT after decode).
    pub fn ingest_min_mip_feedback(&mut self, feedback: &[u8]) {
        let n = (self.feedback_width as usize)
            .saturating_mul(self.feedback_height as usize)
            .min(feedback.len());
        if self.last_feedback.len() < n {
            self.last_feedback.resize(n, SFS_UNUSED_MIP);
        }
        self.last_feedback[..n].copy_from_slice(&feedback[..n]);
    }

    /// Collect tile indices needing residency (budget-limited, finer mips first).
    pub fn tiles_to_stream(&self, max_tiles: u32) -> Vec<u32> {
        if self.last_feedback.is_empty() {
            return Vec::new();
        }

        let budget = max_tiles.min(self.max_tiles_per_frame).max(1) as usize;
        let fw = self.feedback_width.max(1);

        let mut candidates: Vec<(u8, u32)> = Vec::with_capacity(64);
        for (i, &mip) in self.last_feedback.iter().enumerate() {
            if mip == SFS_UNUSED_MIP {
                continue;
            }
            let fx = (i as u32) % fw;
            let fy = (i as u32) / fw;
            let tile = (fy / 2) * ((fw + 1) / 2) + (fx / 2);
            if self.is_resident(tile) {
                continue;
            }
            candidates.push((mip, tile));
        }

        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        candidates.dedup_by_key(|c| c.1);

        let out: Vec<u32> = candidates
            .into_iter()
            .take(budget)
            .map(|(_, t)| t)
            .collect();

        debug!(
            "[SFS] tiles_to_stream requested={} budget={}",
            out.len(),
            budget
        );
        out
    }

    pub fn tiles_to_stream_and_mark(&mut self, max_tiles: u32) -> Vec<u32> {
        let tiles = self.tiles_to_stream(max_tiles);
        for &t in &tiles {
            self.mark_resident(t);
        }
        tiles
    }
}

pub struct SamplerFeedbackStreaming {
    pub enabled: bool,
    pub tracker: SfsResidencyTracker,
    #[cfg(windows)]
    pub feedback_texture: ID3D12Resource,
    #[cfg(windows)]
    pub resolve_buffer: ID3D12Resource,
}

impl SamplerFeedbackStreaming {
    pub fn feedback_width(&self) -> u32 {
        self.tracker.feedback_width
    }

    pub fn feedback_height(&self) -> u32 {
        self.tracker.feedback_height
    }
}

#[cfg(windows)]
pub fn create_sfs(
    device: &Dx12Device,
    atlas: &TiledAtlas,
) -> Dx12Result<SamplerFeedbackStreaming> {
    unsafe {
        let mut opts = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        let ok = device
            .device
            .CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS7,
                &mut opts as *mut _ as *mut _,
                std::mem::size_of_val(&opts) as u32,
            )
            .is_ok();

        if !ok || opts.SamplerFeedbackTier.0 < D3D12_SAMPLER_FEEDBACK_TIER_0_9.0 {
            return Err(Dx12Error::FeatureUnavailable(
                "Sampler Feedback Tier < 0.9".into(),
            ));
        }

        // Half-res feedback keeps resolve cheap on iGPU (Intel GDC guidance).
        let fw = (atlas.width / 2).max(1);
        let fh = (atlas.height / 2).max(1);

        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let feedback_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: fw as u64,
            Height: fh,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_SAMPLER_FEEDBACK_MIN_MIP_OPAQUE,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            Alignment: 0,
        };
        let mut feedback_texture: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &feedback_desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut feedback_texture,
        )?;

        let resolve_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: (fw * fh) as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            Alignment: 0,
        };
        let mut resolve_buffer: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &resolve_desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut resolve_buffer,
        )?;

        info!(
            "[SFS] feedback {}x{} + resolve buffer (stream mips for tiled atlas)",
            fw, fh
        );

        Ok(SamplerFeedbackStreaming {
            enabled: true,
            tracker: SfsResidencyTracker::new(fw, fh, LOW_SPEC_TILES_PER_FRAME),
            feedback_texture: feedback_texture.unwrap(),
            resolve_buffer: resolve_buffer.unwrap(),
        })
    }
}

/// Returns tile indices that need residency based on last ingested feedback.
pub fn tiles_to_stream(sfs: &SamplerFeedbackStreaming, max_tiles: u32) -> Vec<u32> {
    if !sfs.enabled {
        return Vec::new();
    }
    sfs.tracker.tiles_to_stream(max_tiles)
}

pub fn tiles_to_stream_and_mark(
    sfs: &mut SamplerFeedbackStreaming,
    max_tiles: u32,
) -> Vec<u32> {
    if !sfs.enabled {
        return Vec::new();
    }
    sfs.tracker.tiles_to_stream_and_mark(max_tiles)
}

#[cfg(not(windows))]
pub fn create_sfs(_device: &Dx12Device, _atlas: &TiledAtlas) -> Dx12Result<SamplerFeedbackStreaming> {
    Err(Dx12Error::Msg("Windows only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unused_feedback_streams_nothing() {
        let t = SfsResidencyTracker::new(4, 4, 8);
        assert!(t.tiles_to_stream(32).is_empty());
    }

    #[test]
    fn budget_caps_requests() {
        let mut t = SfsResidencyTracker::new(8, 8, LOW_SPEC_TILES_PER_FRAME);
        let mut feedback = vec![SFS_UNUSED_MIP; 64];
        for i in 0..32 {
            feedback[i] = 1;
        }
        t.ingest_min_mip_feedback(&feedback);
        let tiles = t.tiles_to_stream(100);
        assert!(tiles.len() <= LOW_SPEC_TILES_PER_FRAME as usize);
        assert!(!tiles.is_empty());
    }

    #[test]
    fn mark_skips_already_resident() {
        let mut t = SfsResidencyTracker::new(4, 4, 8);
        t.ingest_min_mip_feedback(&[0u8; 16]);
        let first = t.tiles_to_stream_and_mark(8);
        assert!(!first.is_empty());
        let second = t.tiles_to_stream(8);
        for tile in &second {
            assert!(!first.contains(tile));
        }
    }
}
