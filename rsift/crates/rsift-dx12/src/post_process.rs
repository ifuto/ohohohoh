//! Phase 6 — Post-processing (CMAA2, VRS, f16).
//!
//! CMAA2 (Intel): morphological AA with low blur — better than FXAA for sharpness,
//! cheaper than TAA/SMAA on integrated GPUs when quality preset is 0–1.

use crate::device::Dx12Device;
use crate::error::Dx12Result;
use rsift_api::adaptive_perf::PerformanceTier;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

pub struct VariableRateShadingMap {
    #[cfg(windows)]
    pub resource: ID3D12Resource,
}

impl VariableRateShadingMap {
    #[cfg(windows)]
    pub fn new(device: &Dx12Device, width: u32, height: u32) -> Dx12Result<Self> {
        unsafe {
            // Tier 2 VRS uses a 16×16 tile size typically.
            let vrs_width = (width + 15) / 16;
            let vrs_height = (height + 15) / 16;

            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
                CreationNodeMask: 0,
                VisibleNodeMask: 0,
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: vrs_width as u64,
                Height: vrs_height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8_UINT,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            };

            let mut resource: Option<ID3D12Resource> = None;
            device.device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                None,
                &mut resource,
            )?;

            Ok(Self {
                resource: resource.unwrap(),
            })
        }
    }
}

/// Intel CMAA2 quality preset (`CMAA2_STATIC_QUALITY_PRESET` 0..=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Cmaa2Quality {
    /// Fastest — integrated / Minimal tier.
    Lowest = 0,
    Low = 1,
    Medium = 2,
    High = 3,
}

/// Conservative Morphological Anti-Aliasing 2.0 working set.
///
/// Matches Intel sample structure: edge detect → process candidates → deferred apply,
/// with DispatchIndirect so empty tiles cost almost nothing (good for low-end).
#[derive(Debug, Clone)]
pub struct Cmaa2State {
    pub enabled: bool,
    pub quality: Cmaa2Quality,
    /// Extra sharpness path (`CMAA2_EXTRA_SHARPNESS`) — prefer on low-end (less blur).
    pub extra_sharpness: bool,
    pub width: u32,
    pub height: u32,
    /// Edge candidate capacity (uints). Sized from resolution × quality.
    pub edge_candidate_capacity: u32,
    /// Working buffer bytes estimate (no GPU alloc until `create_resources`).
    pub working_set_bytes: u64,
}

impl Cmaa2State {
    pub fn for_tier(tier: PerformanceTier, width: u32, height: u32) -> Self {
        let (enabled, quality, extra_sharpness) = match tier {
            // Minimal: skip post AA entirely (biggest win on iGPU).
            PerformanceTier::Minimal => (false, Cmaa2Quality::Lowest, true),
            PerformanceTier::Low => (true, Cmaa2Quality::Lowest, true),
            PerformanceTier::Medium => (true, Cmaa2Quality::Low, true),
            PerformanceTier::High => (true, Cmaa2Quality::Medium, false),
        };
        Self::with_params(enabled, quality, extra_sharpness, width, height)
    }

    pub fn with_params(
        enabled: bool,
        quality: Cmaa2Quality,
        extra_sharpness: bool,
        width: u32,
        height: u32,
    ) -> Self {
        // Intel sample: edge buffer scales with pixel count; quality bumps headroom.
        let pixels = width as u64 * height as u64;
        let headroom = match quality {
            Cmaa2Quality::Lowest => 1,
            Cmaa2Quality::Low => 2,
            Cmaa2Quality::Medium => 3,
            Cmaa2Quality::High => 4,
        };
        // ~1 candidate slot per 8–32 pixels depending on quality.
        let edge_candidate_capacity = ((pixels / (32 / headroom)).max(1024) as u32).min(1 << 20);
        let working_set_bytes = if enabled {
            // Color in-place + edge flags + candidate list + indirect args (~few MB).
            pixels // edge flags (R8)
                + edge_candidate_capacity as u64 * 8
                + 256
        } else {
            0
        };
        Self {
            enabled,
            quality,
            extra_sharpness,
            width,
            height,
            edge_candidate_capacity,
            working_set_bytes,
        }
    }

    /// Kernel dispatch plan (threadgroups). Empty when disabled.
    pub fn edge_detect_groups(&self) -> (u32, u32) {
        if !self.enabled {
            return (0, 0);
        }
        // EdgesColor2x2CS processes 2×2 blocks with 16×16 threads → 32×32 pixels/group.
        let gx = (self.width + 31) / 32;
        let gy = (self.height + 31) / 32;
        (gx, gy)
    }
}

/// Prefer f16 math in post when GPU supports it (bandwidth win on weak iGPUs).
pub const USE_F16_MATH: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_disables_cmaa() {
        let s = Cmaa2State::for_tier(PerformanceTier::Minimal, 1280, 720);
        assert!(!s.enabled);
        assert_eq!(s.working_set_bytes, 0);
    }

    #[test]
    fn low_uses_fast_preset() {
        let s = Cmaa2State::for_tier(PerformanceTier::Low, 1920, 1080);
        assert!(s.enabled);
        assert_eq!(s.quality, Cmaa2Quality::Lowest);
        assert!(s.extra_sharpness);
    }
}
