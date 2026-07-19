//! Phase 3 — Tiled / Reserved Resources (sparse virtual textures, chunk pages).

use crate::device::Dx12Device;
use crate::error::{Dx12Error, Dx12Result};
use crate::win::{
    D3D12_RESOURCE_FLAG_ALLOW_SAMPLER_FEEDBACK, DXGI_FORMAT_BC7_UNORM, DXGI_SAMPLE_DESC,
};
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

/// One 64KB tile page per chunk region (16³ blocks atlas page).
pub const TILE_SIZE_BYTES: u64 = 64 * 1024;
pub const CHUNK_PAGE_COUNT: u32 = 4096;

pub struct TiledAtlas {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub reserved: bool,
    #[cfg(windows)]
    pub texture: ID3D12Resource,
    #[cfg(windows)]
    pub tile_mappings: Vec<D3D12_TILE_RANGE_FLAGS>,
}

#[cfg(windows)]
pub fn create_reserved_texture(
    device: &Dx12Device,
    width: u32,
    height: u32,
    mips: u32,
) -> Dx12Result<TiledAtlas> {
    unsafe {
        let mut tier = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        device
            .device
            .CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS,
                &mut tier as *mut _ as *mut _,
                std::mem::size_of_val(&tier) as u32,
            )
            .map_err(|e| Dx12Error::Msg(format!("CheckFeatureSupport: {}", e)))?;

        if tier.TiledResourcesTier.0 < D3D12_TILED_RESOURCES_TIER_2.0 {
            return Err(Dx12Error::FeatureUnavailable(
                "Tiled resources tier < 2".into(),
            ));
        }

        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: mips as u16,
            Format: DXGI_FORMAT_BC7_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_SAMPLER_FEEDBACK,
            Alignment: 0,
        };

        let mut texture: Option<ID3D12Resource> = None;
        device
            .device
            .CreateReservedResource(&desc, D3D12_RESOURCE_STATE_COMMON, None, &mut texture)?;

        info!(
            "[TiledResources] reserved BC7 {}x{} mips={} (SFS-capable)",
            width, height, mips
        );

        Ok(TiledAtlas {
            width,
            height,
            mip_levels: mips,
            reserved: true,
            texture: texture.unwrap(),
            tile_mappings: vec![D3D12_TILE_RANGE_FLAG_NONE; CHUNK_PAGE_COUNT as usize],
        })
    }
}

#[cfg(windows)]
pub fn map_chunk_tile(
    device: &Dx12Device,
    atlas: &mut TiledAtlas,
    tile_index: u32,
    heap: &ID3D12Heap,
    heap_offset: u64,
) -> Dx12Result<()> {
    unsafe {
        let range = D3D12_TILED_RESOURCE_COORDINATE {
            X: tile_index % 64,
            Y: tile_index / 64,
            Z: 0,
            Subresource: 0,
        };
        let tile_range = D3D12_TILE_REGION_SIZE {
            NumTiles: 1,
            UseBox: false.into(),
            ..Default::default()
        };
        let heap_range = D3D12_TILE_RANGE_FLAG_NONE;
        let heap_start = (heap_offset / (64 * 1024)) as u32;
        device.queue.UpdateTileMappings(
            &atlas.texture,
            1,
            Some(&range),
            Some(&tile_range),
            heap,
            1,
            Some(&heap_range),
            Some(&heap_start),
            None,
            D3D12_TILE_MAPPING_FLAG_NONE,
        );
        if (tile_index as usize) < atlas.tile_mappings.len() {
            atlas.tile_mappings[tile_index as usize] = heap_range;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub struct TiledAtlas;

#[cfg(not(windows))]
pub fn create_reserved_texture(
    _device: &Dx12Device,
    _width: u32,
    _height: u32,
    _mips: u32,
) -> Dx12Result<TiledAtlas> {
    Err(Dx12Error::Msg("Windows only".into()))
}
