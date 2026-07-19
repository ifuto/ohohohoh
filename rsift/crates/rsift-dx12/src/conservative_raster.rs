//! Phase 4 — Conservative Rasterization (SM 6.6+, overestimation for voxel cracks).

use crate::device::Dx12Device;
use crate::error::{Dx12Error, Dx12Result};
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConservativeTier {
    Off,
    Tier1,
    Tier3,
}

pub struct ConservativeRasterState {
    pub tier: ConservativeTier,
    pub overestimate: bool,
    pub post_snap: bool,
}

impl ConservativeRasterState {
    pub fn probe(device: &Dx12Device) -> Dx12Result<Self> {
        #[cfg(windows)]
        unsafe {
            let mut opts = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
            device
                .device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS,
                    &mut opts as *mut _ as *mut _,
                    std::mem::size_of_val(&opts) as u32,
                )
                .map_err(|e| Dx12Error::Msg(format!("{}", e)))?;

            let tier = match opts.ConservativeRasterizationTier {
                D3D12_CONSERVATIVE_RASTERIZATION_TIER_3 => ConservativeTier::Tier3,
                D3D12_CONSERVATIVE_RASTERIZATION_TIER_1 => ConservativeTier::Tier1,
                _ => ConservativeTier::Off,
            };

            info!("[ConservativeRaster] tier={:?}", tier);

            Ok(Self {
                tier,
                overestimate: tier != ConservativeTier::Off,
                post_snap: tier == ConservativeTier::Tier3,
            })
        }
        #[cfg(not(windows))]
        Err(Dx12Error::Msg("Windows only".into()))
    }

    #[cfg(windows)]
    pub fn rasterizer_desc(&self) -> D3D12_RASTERIZER_DESC {
        let mode = if self.overestimate {
            D3D12_CONSERVATIVE_RASTERIZATION_MODE_ON
        } else {
            D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF
        };
        D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_BACK,
            ConservativeRaster: mode,
            ..Default::default()
        }
    }
}
