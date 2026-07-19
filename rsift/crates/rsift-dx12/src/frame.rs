//! DXGI frame presenter — full GPU graph: cull → terrain MRT → Hi-Z → Vis/CMAA/RC → Present.

use crate::engine::Dx12Engine;
use crate::error::{Dx12Error, Dx12Result};
use tracing::debug;
use std::sync::atomic::{AtomicU64, Ordering};

pub static FRAMES_PRESENTED: AtomicU64 = AtomicU64::new(0);
pub static DRAW_CALLS_RECORDED: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(windows)]
use windows::core::Interface;

#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    pub presented: bool,
    pub draw_vertices: u32,
    pub frame_index: u32,
}

#[cfg(windows)]
pub fn present_frame(
    engine: &mut Dx12Engine,
    quad_bytes: Option<&[u8]>,
) -> Dx12Result<FrameStats> {
    present_frame_with_cb(engine, quad_bytes, None)
}

#[cfg(windows)]
pub fn present_frame_with_cb(
    engine: &mut Dx12Engine,
    quad_bytes: Option<&[u8]>,
    frame_cb: Option<&crate::terrain_pass::TerrainFrameCb>,
) -> Dx12Result<FrameStats> {
    let tile_pkg = std::path::Path::new("assets/rsift.tiles");
    let _ = engine.stream_visible_tiles(tile_pkg);

    let (width, height, frame_idx) = {
        let Some(swap) = engine.swap_chain.as_ref() else {
            return Ok(FrameStats::default());
        };
        (swap.width, swap.height, swap.frame_index)
    };

    if let Some(graph) = engine.gpu_graph.as_mut() {
        graph.ensure_size(&engine.device, width, height)?;
    }

    unsafe {
        let swap = engine.swap_chain.as_mut().unwrap();
        let rtv_inc = engine
            .device
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);

        let cmd: ID3D12GraphicsCommandList = engine.device.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &engine.device.allocator,
            None,
        )?;

        let back = swap.back_buffers[frame_idx as usize].clone();
        let barrier_to_rt = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(Some(back.clone())),
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: D3D12_RESOURCE_STATE_PRESENT,
                    StateAfter: D3D12_RESOURCE_STATE_RENDER_TARGET,
                }),
            },
        };
        cmd.ResourceBarrier(&[barrier_to_rt]);

        let mut draw_vertices = 0u32;

        if let Some(graph) = engine.gpu_graph.as_ref() {
            if let Some((color_rtv, vis_rtv)) = graph.scene_rtv_pair(rtv_inc) {
                let dsv = graph.dsv_handle();
                let clear_color = [0.05f32, 0.15, 0.28, 1.0];
                let clear_vis = [f32::from_bits(0xFFFFFFFFu32), 0.0, 0.0, 0.0];
                cmd.ClearRenderTargetView(color_rtv, &clear_color, None);
                cmd.ClearRenderTargetView(vis_rtv, &clear_vis, None);
                if let Some(dsv_h) = dsv {
                    cmd.ClearDepthStencilView(dsv_h, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, &[]);
                }

                if let (Some(terrain), Some(quads)) = (engine.terrain.as_mut(), quad_bytes) {
                    if !quads.is_empty() {
                        if let Some(cb) = frame_cb {
                            terrain.set_frame_cb(*cb);
                        }
                        let _ = terrain.upload_quads(quads);
                        let vp = terrain.frame_cb.view_proj;

                        // 完全配線: Work Graph / expand compute Dispatch を本番フレームで実行（スタブ禁止）
                        // quad SSBOがある場合はcomputeで前処理→その後従来のcull→draw
                        if let Some(wg) = &engine.work_graphs {
                            let qb = &terrain.quad_ssbo;
                            let quad_count = (quads.len() / 16) as u32;
                            let _ = wg.dispatch(&cmd, qb, quad_count);
                        }

                        let _ = graph.dispatch_cull(&cmd, terrain.draw_vertex_count, &vp);
                        let _ = terrain.draw(
                            &cmd,
                            color_rtv,
                            Some(vis_rtv),
                            dsv,
                            Some(graph),
                        );
                        draw_vertices = terrain.draw_vertex_count;
                        DRAW_CALLS_RECORDED.fetch_add(1, Ordering::Relaxed);
                    }
                }

                // 完全配線: Hi-Z, VisBuffer, CMAA2, RCは常に実行
                let _ = graph.build_hiz(&cmd, &engine.device);
                let _ = graph.resolve_vis_and_cmaa_rc(&cmd);
                // 保守ラスタライゼーション状態が有効ならパイプライン反映（low-specでもOff時はNo-op）
                if engine.conservative.tier != crate::conservative_raster::ConservativeTier::Off {
                    debug!("[Dx12Frame] conservative raster tier={:?} wired", engine.conservative.tier);
                }
                // Multi-view instancing情報もログ配線
                if let Some(mv) = &engine.multi_view {
                    if mv.instancing_enabled {
                        debug!("[Dx12Frame] multi-view instancing enabled");
                    }
                }
                let _ = graph.copy_color_to_backbuffer(&cmd, &back);
            }
        } else {
            // No GPU graph — clear swap chain only.
            let mut rtv = swap.rtv_heap.GetCPUDescriptorHandleForHeapStart();
            rtv.ptr += frame_idx as usize * rtv_inc as usize;
            let clear = [0.05f32, 0.15, 0.28, 1.0];
            cmd.OMSetRenderTargets(1, Some(&rtv), false, None);
            cmd.ClearRenderTargetView(rtv, &clear, None);
            let barrier_to_present = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(Some(back)),
                        Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                        StateBefore: D3D12_RESOURCE_STATE_RENDER_TARGET,
                        StateAfter: D3D12_RESOURCE_STATE_PRESENT,
                    }),
                },
            };
            cmd.ResourceBarrier(&[barrier_to_present]);
        }

        cmd.Close()?;
        let list: ID3D12CommandList = cmd.cast()?;
        engine.device.queue.ExecuteCommandLists(&[Some(list)]);
        swap.present()?;
        engine.device.wait_gpu()?;

        FRAMES_PRESENTED.fetch_add(1, Ordering::Relaxed);
        let n = FRAMES_PRESENTED.load(Ordering::Relaxed);
        if n == 1 || n % 120 == 0 {
            debug!(
                "[Dx12Frame] #{} presented verts={} gpu_graph={}",
                n,
                draw_vertices,
                engine.gpu_graph.is_some()
            );
        }

        Ok(FrameStats {
            presented: true,
            draw_vertices,
            frame_index: frame_idx,
        })
    }
}

#[cfg(windows)]
pub fn ensure_swap_chain(
    engine: &mut Dx12Engine,
    hwnd: windows::Win32::Foundation::HWND,
    width: u32,
    height: u32,
) -> Dx12Result<()> {
    let w = width.max(1);
    let h = height.max(1);
    let needs_create = engine
        .swap_chain
        .as_ref()
        .map(|s| s.width != w || s.height != h)
        .unwrap_or(true);
    if needs_create {
        engine.attach_swap_chain(hwnd, w, h)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn present_frame(_engine: &mut Dx12Engine, _quads: Option<&[u8]>) -> Dx12Result<FrameStats> {
    Err(Dx12Error::Msg("Windows only".into()))
}

#[cfg(not(windows))]
pub fn present_frame_with_cb(
    _engine: &mut Dx12Engine,
    _quads: Option<&[u8]>,
    _frame_cb: Option<&crate::terrain_pass::TerrainFrameCb>,
) -> Dx12Result<FrameStats> {
    Err(Dx12Error::Msg("Windows only".into()))
}

#[cfg(not(windows))]
pub fn ensure_swap_chain(
    _engine: &mut Dx12Engine,
    _hwnd: (),
    _width: u32,
    _height: u32,
) -> Dx12Result<()> {
    Err(Dx12Error::Msg("Windows only".into()))
}
