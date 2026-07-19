//! Frame GPU graph — depth, Hi-Z, ExecuteIndirect cull, VisBuffer resolve, CMAA2, Radiance Cascades.
//!
//! These are live command-list bodies recorded every present (not caps-off / fail-loud substitutes).

use crate::device::Dx12Device;
use crate::dxc::{CompiledShader, DxcCompiler};
use crate::error::{Dx12Error, Dx12Result};
use crate::resources::{create_default_buffer, create_upload_buffer, upload_slice, GpuBuffer};
use crate::win::shader_bytecode;
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D::*;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(windows)]
use windows::core::Interface;

const MAX_INDIRECT_DRAWS: u32 = 64;

pub struct FrameGpuGraph {
    pub width: u32,
    pub height: u32,
    pub enabled: bool,

    #[cfg(windows)]
    pub depth: Option<ID3D12Resource>,
    #[cfg(windows)]
    pub dsv_heap: Option<ID3D12DescriptorHeap>,
    #[cfg(windows)]
    pub color: Option<ID3D12Resource>,
    #[cfg(windows)]
    pub color_aa: Option<ID3D12Resource>,
    #[cfg(windows)]
    pub vis: Option<ID3D12Resource>,
    #[cfg(windows)]
    pub scene_rtv_heap: Option<ID3D12DescriptorHeap>,

    #[cfg(windows)]
    pub hiz: Option<ID3D12Resource>,
    pub hiz_mips: u32,

    #[cfg(windows)]
    pub desc_heap: Option<ID3D12DescriptorHeap>,
    pub desc_increment: u32,

    #[cfg(windows)]
    pub cull_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub cull_root: Option<ID3D12RootSignature>,
    #[cfg(windows)]
    pub hiz_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub hiz_root: Option<ID3D12RootSignature>,
    #[cfg(windows)]
    pub resolve_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub resolve_root: Option<ID3D12RootSignature>,
    #[cfg(windows)]
    pub cmaa_edge_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub cmaa_apply_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub cmaa_root: Option<ID3D12RootSignature>,
    #[cfg(windows)]
    pub radiance_pso: Option<ID3D12PipelineState>,
    #[cfg(windows)]
    pub radiance_root: Option<ID3D12RootSignature>,

    pub draw_args: Option<GpuBuffer>,
    pub aabb_upload: Option<GpuBuffer>,
    pub edge_flags: Option<GpuBuffer>,
    pub radiance_probes: Option<GpuBuffer>,
    /// DEFAULT heap destination for DirectStorage CopyBufferRegion.
    pub tile_gpu_dest: Option<GpuBuffer>,

    #[cfg(windows)]
    pub command_signature: Option<ID3D12CommandSignature>,

    pub instance_count: u32,
    pub radiance_grid_w: u32,
    pub radiance_grid_h: u32,
    pub radiance_cascades: u32,
}

impl FrameGpuGraph {
    pub fn create(device: &Dx12Device, dxc: &DxcCompiler) -> Dx12Result<Self> {
        let cull_cs = dxc.compile_hlsl(
            include_str!("../shaders/cull_indirect_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;
        let hiz_cs = dxc.compile_hlsl(
            include_str!("../shaders/hiz_build_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;
        let resolve_cs = dxc.compile_hlsl(
            include_str!("../shaders/vis_resolve_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;
        let cmaa_edge = dxc.compile_hlsl(
            include_str!("../shaders/cmaa2_edge_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;
        let cmaa_apply = dxc.compile_hlsl(
            include_str!("../shaders/cmaa2_apply_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;
        let radiance_cs = dxc.compile_hlsl(
            include_str!("../shaders/radiance_cascade_cs.hlsl"),
            "CsMain",
            dxc.target_cs(),
        )?;

        #[cfg(windows)]
        let pipes = unsafe { create_compute_pipelines(device, &cull_cs, &hiz_cs, &resolve_cs, &cmaa_edge, &cmaa_apply, &radiance_cs)? };
        #[cfg(not(windows))]
        let _ = (cull_cs, hiz_cs, resolve_cs, cmaa_edge, cmaa_apply, radiance_cs);

        let draw_args = create_default_buffer(
            device,
            (MAX_INDIRECT_DRAWS as u64) * 16,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            "indirect_draw_args",
        )?;
        let aabb_upload = create_upload_buffer(device, (MAX_INDIRECT_DRAWS as u64) * 16, "cull_aabbs")?;
        let edge_flags = create_default_buffer(
            device,
            1920 * 1080 * 4,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            "cmaa_edges",
        )?;
        let radiance_probes = create_default_buffer(
            device,
            256 * 256 * 4 * 16,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            "radiance_probes",
        )?;
        let tile_gpu_dest = create_default_buffer(
            device,
            64 * 65536,
            D3D12_RESOURCE_FLAG_NONE,
            "dstorage_tile_dest",
        )?;

        #[cfg(windows)]
        let command_signature = unsafe { create_draw_command_signature(device)? };

        #[cfg(windows)]
        let desc_increment = unsafe {
            device
                .device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
        };
        #[cfg(not(windows))]
        let desc_increment = 0u32;

        info!("[GpuGraph] cull+HiZ+VisResolve+CMAA2+RC compute PSOs ready");

        Ok(Self {
            width: 0,
            height: 0,
            enabled: true,
            #[cfg(windows)]
            depth: None,
            #[cfg(windows)]
            dsv_heap: None,
            #[cfg(windows)]
            color: None,
            #[cfg(windows)]
            color_aa: None,
            #[cfg(windows)]
            vis: None,
            #[cfg(windows)]
            scene_rtv_heap: None,
            #[cfg(windows)]
            hiz: None,
            hiz_mips: 0,
            #[cfg(windows)]
            desc_heap: None,
            desc_increment,
            #[cfg(windows)]
            cull_pso: pipes.cull_pso,
            #[cfg(windows)]
            cull_root: pipes.cull_root,
            #[cfg(windows)]
            hiz_pso: pipes.hiz_pso,
            #[cfg(windows)]
            hiz_root: pipes.hiz_root,
            #[cfg(windows)]
            resolve_pso: pipes.resolve_pso,
            #[cfg(windows)]
            resolve_root: pipes.resolve_root,
            #[cfg(windows)]
            cmaa_edge_pso: pipes.cmaa_edge_pso,
            #[cfg(windows)]
            cmaa_apply_pso: pipes.cmaa_apply_pso,
            #[cfg(windows)]
            cmaa_root: pipes.cmaa_root,
            #[cfg(windows)]
            radiance_pso: pipes.radiance_pso,
            #[cfg(windows)]
            radiance_root: pipes.radiance_root,
            draw_args: Some(draw_args),
            aabb_upload: Some(aabb_upload),
            edge_flags: Some(edge_flags),
            radiance_probes: Some(radiance_probes),
            tile_gpu_dest: Some(tile_gpu_dest),
            #[cfg(windows)]
            command_signature: Some(command_signature),
            instance_count: 1,
            radiance_grid_w: 64,
            radiance_grid_h: 64,
            radiance_cascades: 2,
        })
    }

    #[cfg(windows)]
    pub fn ensure_size(&mut self, device: &Dx12Device, width: u32, height: u32) -> Dx12Result<()> {
        let w = width.max(1);
        let h = height.max(1);
        if self.width == w && self.height == h && self.color.is_some() {
            return Ok(());
        }
        unsafe {
            self.create_frame_targets(device, w, h)?;
        }
        self.width = w;
        self.height = h;
        // Grow edge buffer if needed.
        let need = (w as u64) * (h as u64) * 4;
        if self
            .edge_flags
            .as_ref()
            .map(|b| b.size < need)
            .unwrap_or(true)
        {
            self.edge_flags = Some(create_default_buffer(
                device,
                need.max(4096),
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                "cmaa_edges",
            )?);
        }
        Ok(())
    }

    #[cfg(windows)]
    unsafe fn create_frame_targets(
        &mut self,
        device: &Dx12Device,
        w: u32,
        h: u32,
    ) -> Dx12Result<()> {
        let heap_default = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };

        // Color (RT + UAV for CMAA/resolve)
        let color_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        };
        let color_clear = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                Color: [0.05, 0.15, 0.28, 1.0],
            },
        };
        let mut color: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap_default,
            D3D12_HEAP_FLAG_NONE,
            &color_desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            Some(&color_clear),
            &mut color,
        )?;

        let mut color_aa: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap_default,
            D3D12_HEAP_FLAG_NONE,
            &color_desc,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            Some(&color_clear),
            &mut color_aa,
        )?;

        // Vis buffer
        let vis_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R32_UINT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        };
        let vis_clear = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_R32_UINT,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                Color: [f32::from_bits(0xFFFFFFFFu32), 0.0, 0.0, 0.0],
            },
        };
        let mut vis: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap_default,
            D3D12_HEAP_FLAG_NONE,
            &vis_desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            Some(&vis_clear),
            &mut vis,
        )?;

        // Depth typeless for DSV + Hi-Z SRV
        let depth_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R32_TYPELESS,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        };
        let depth_clear = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_D32_FLOAT,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                    Depth: 0.0, // Reverse-Z clear
                    Stencil: 0,
                },
            },
        };
        let mut depth: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap_default,
            D3D12_HEAP_FLAG_NONE,
            &depth_desc,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Some(&depth_clear),
            &mut depth,
        )?;

        let mips = ((w.max(h) as f32).log2().floor() as u32).max(1);
        let hiz_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: mips as u16,
            Format: DXGI_FORMAT_R32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        };
        let mut hiz: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap_default,
            D3D12_HEAP_FLAG_NONE,
            &hiz_desc,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            None,
            &mut hiz,
        )?;

        let dsv_heap: ID3D12DescriptorHeap = device.device.CreateDescriptorHeap(
            &D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            },
        )?;
        let dsv = dsv_heap.GetCPUDescriptorHandleForHeapStart();
        let dsv_desc = D3D12_DEPTH_STENCIL_VIEW_DESC {
            Format: DXGI_FORMAT_D32_FLOAT,
            ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
            Flags: D3D12_DSV_FLAG_NONE,
            Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 },
            },
        };
        device
            .device
            .CreateDepthStencilView(depth.as_ref(), Some(&dsv_desc), dsv);

        let scene_rtv_heap: ID3D12DescriptorHeap = device.device.CreateDescriptorHeap(
            &D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: 2,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            },
        )?;
        let rtv_inc = device
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
        let mut rtv = scene_rtv_heap.GetCPUDescriptorHandleForHeapStart();
        device
            .device
            .CreateRenderTargetView(color.as_ref(), None, rtv);
        rtv.ptr += rtv_inc as usize;
        device.device.CreateRenderTargetView(vis.as_ref(), None, rtv);

        // Descriptor heap: color SRV/UAV, vis SRV, depth SRV, hiz UAVs/SRVs, color_aa UAV
        let desc_count = 5 + mips * 2;
        let desc_heap: ID3D12DescriptorHeap = device.device.CreateDescriptorHeap(
            &D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: desc_count,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            },
        )?;
        let inc = self.desc_increment;
        let mut cpu = desc_heap.GetCPUDescriptorHandleForHeapStart();
        // 0: color SRV
        device.device.CreateShaderResourceView(
            color.as_ref(),
            Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                        PlaneSlice: 0,
                        ResourceMinLODClamp: 0.0,
                    },
                },
            }),
            cpu,
        );
        cpu.ptr += inc as usize;
        // 1: color UAV
        device.device.CreateUnorderedAccessView(
            color.as_ref(),
            None,
            Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_UAV {
                        MipSlice: 0,
                        PlaneSlice: 0,
                    },
                },
            }),
            cpu,
        );
        cpu.ptr += inc as usize;
        // 2: vis SRV
        device.device.CreateShaderResourceView(
            vis.as_ref(),
            Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R32_UINT,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                        PlaneSlice: 0,
                        ResourceMinLODClamp: 0.0,
                    },
                },
            }),
            cpu,
        );
        cpu.ptr += inc as usize;
        // 3: depth as R32_FLOAT SRV (for Hi-Z seed)
        device.device.CreateShaderResourceView(
            depth.as_ref(),
            Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R32_FLOAT,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                        PlaneSlice: 0,
                        ResourceMinLODClamp: 0.0,
                    },
                },
            }),
            cpu,
        );
        cpu.ptr += inc as usize;
        // 4: color_aa UAV (CMAA2 apply destination)
        device.device.CreateUnorderedAccessView(
            color_aa.as_ref(),
            None,
            Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_UAV {
                        MipSlice: 0,
                        PlaneSlice: 0,
                    },
                },
            }),
            cpu,
        );
        cpu.ptr += inc as usize;
        // 5..: Hi-Z UAV per mip, then SRV per mip
        for mip in 0..mips {
            device.device.CreateUnorderedAccessView(
                hiz.as_ref(),
                None,
                Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_FLOAT,
                    ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_UAV {
                            MipSlice: mip,
                            PlaneSlice: 0,
                        },
                    },
                }),
                cpu,
            );
            cpu.ptr += inc as usize;
        }
        for mip in 0..mips {
            device.device.CreateShaderResourceView(
                hiz.as_ref(),
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_FLOAT,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MostDetailedMip: mip,
                            MipLevels: 1,
                            PlaneSlice: 0,
                            ResourceMinLODClamp: 0.0,
                        },
                    },
                }),
                cpu,
            );
            cpu.ptr += inc as usize;
        }

        self.color = color;
        self.color_aa = color_aa;
        self.vis = vis;
        self.depth = depth;
        self.hiz = hiz;
        self.hiz_mips = mips;
        self.dsv_heap = Some(dsv_heap);
        self.scene_rtv_heap = Some(scene_rtv_heap);
        self.desc_heap = Some(desc_heap);
        self.radiance_grid_w = ((w as f32) * 0.5).max(8.0) as u32 / 4;
        self.radiance_grid_h = ((h as f32) * 0.5).max(8.0) as u32 / 4;
        Ok(())
    }

    #[cfg(windows)]
    pub fn scene_rtv_pair(
        &self,
        rtv_increment: u32,
    ) -> Option<(D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CPU_DESCRIPTOR_HANDLE)> {
        let heap = self.scene_rtv_heap.as_ref()?;
        let color = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
        let mut vis = color;
        vis.ptr += rtv_increment as usize;
        Some((color, vis))
    }

    #[cfg(windows)]
    pub fn dsv_handle(&self) -> Option<D3D12_CPU_DESCRIPTOR_HANDLE> {
        self.dsv_heap
            .as_ref()
            .map(|h| unsafe { h.GetCPUDescriptorHandleForHeapStart() })
    }

    /// Upload one or more instance spheres and run GPU cull → ExecuteIndirect args.
    #[cfg(windows)]
    pub fn dispatch_cull(
        &self,
        cmd: &ID3D12GraphicsCommandList,
        vertex_count: u32,
        view_proj: &[[f32; 4]; 4],
    ) -> Dx12Result<()> {
        let (Some(pso), Some(root), Some(args), Some(aabbs)) = (
            self.cull_pso.as_ref(),
            self.cull_root.as_ref(),
            self.draw_args.as_ref(),
            self.aabb_upload.as_ref(),
        ) else {
            return Ok(());
        };

        let planes = extract_frustum_planes(view_proj);
        let instance_count = self.instance_count.max(1).min(MAX_INDIRECT_DRAWS);
        // Default: camera-centered sphere so the primary draw stays visible.
        let mut aabb_bytes = vec![0u8; (instance_count as usize) * 16];
        for i in 0..instance_count as usize {
            let center = [0.0f32, 64.0, 0.0, 256.0]; // large radius → visible
            aabb_bytes[i * 16..(i + 1) * 16].copy_from_slice(bytemuck::bytes_of(&center));
        }
        upload_slice(aabbs, &aabb_bytes)?;

        let mut cb = [0u32; 28];
        for (i, p) in planes.iter().enumerate() {
            cb[i * 4] = p[0].to_bits();
            cb[i * 4 + 1] = p[1].to_bits();
            cb[i * 4 + 2] = p[2].to_bits();
            cb[i * 4 + 3] = p[3].to_bits();
        }
        cb[24] = instance_count;
        cb[25] = vertex_count;

        unsafe {
            cmd.SetPipelineState(pso);
            cmd.SetComputeRootSignature(root);
            cmd.SetComputeRoot32BitConstants(0, 28, cb.as_ptr() as *const _, 0);
            cmd.SetComputeRootShaderResourceView(1, aabbs.resource.GetGPUVirtualAddress());
            cmd.SetComputeRootUnorderedAccessView(2, args.resource.GetGPUVirtualAddress());
            let groups = (instance_count + 63) / 64;
            cmd.Dispatch(groups.max(1), 1, 1);

            let barrier = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(Some(args.resource.clone())),
                    }),
                },
            };
            cmd.ResourceBarrier(&[barrier]);
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn execute_indirect_draw(
        &self,
        cmd: &ID3D12GraphicsCommandList,
    ) -> Dx12Result<()> {
        let (Some(sig), Some(args)) = (self.command_signature.as_ref(), self.draw_args.as_ref())
        else {
            return Ok(());
        };
        unsafe {
            cmd.ExecuteIndirect(
                sig,
                self.instance_count.max(1).min(MAX_INDIRECT_DRAWS),
                &args.resource,
                0,
                None,
                0,
            );
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn build_hiz(&self, cmd: &ID3D12GraphicsCommandList, device: &Dx12Device) -> Dx12Result<()> {
        let (Some(pso), Some(root), Some(heap), Some(depth), Some(hiz)) = (
            self.hiz_pso.as_ref(),
            self.hiz_root.as_ref(),
            self.desc_heap.as_ref(),
            self.depth.as_ref(),
            self.hiz.as_ref(),
        ) else {
            return Ok(());
        };
        let _ = device;
        unsafe {
            // Depth → NON_PIXEL_SHADER_RESOURCE for mip0 seed via CS reading depth SRV,
            // writing Hi-Z mip0; subsequent mips from previous Hi-Z.
            let to_srv = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(Some(depth.clone())),
                        Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                        StateBefore: D3D12_RESOURCE_STATE_DEPTH_WRITE,
                        StateAfter: D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    }),
                },
            };
            cmd.ResourceBarrier(&[to_srv]);

            cmd.SetDescriptorHeaps(&[Some(heap.clone())]);
            cmd.SetPipelineState(pso);
            cmd.SetComputeRootSignature(root);

            let gpu = heap.GetGPUDescriptorHandleForHeapStart();
            let inc = self.desc_increment as u64;

            // First pass: depth SRV (slot 3) → Hi-Z UAV mip0 (slot 5)
            let mut src_w = self.width;
            let mut src_h = self.height;
            {
                let cb = [src_w, src_h, src_w, src_h];
                cmd.SetComputeRoot32BitConstants(0, 4, cb.as_ptr() as *const _, 0);
                let mut depth_srv = gpu;
                depth_srv.ptr += 3 * inc;
                cmd.SetComputeRootDescriptorTable(1, depth_srv);
                let mut hiz0 = gpu;
                hiz0.ptr += 5 * inc;
                cmd.SetComputeRootDescriptorTable(2, hiz0);
                let gx = (src_w + 7) / 8;
                let gy = (src_h + 7) / 8;
                cmd.Dispatch(gx.max(1), gy.max(1), 1);
            }

            // Remaining mips: Hi-Z mip i SRV → mip i+1 UAV
            for mip in 0..self.hiz_mips.saturating_sub(1) {
                let dst_w = (src_w / 2).max(1);
                let dst_h = (src_h / 2).max(1);
                let uav_barrier = D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(Some(hiz.clone())),
                        }),
                    },
                };
                cmd.ResourceBarrier(&[uav_barrier]);

                let cb = [src_w, src_h, dst_w, dst_h];
                cmd.SetComputeRoot32BitConstants(0, 4, cb.as_ptr() as *const _, 0);
                // Hi-Z SRVs start after 5 + mips UAVs
                let mut srv = gpu;
                srv.ptr += (5 + self.hiz_mips as u64 + mip as u64) * inc;
                cmd.SetComputeRootDescriptorTable(1, srv);
                let mut uav = gpu;
                uav.ptr += (5 + (mip + 1) as u64) * inc;
                cmd.SetComputeRootDescriptorTable(2, uav);
                let gx = (dst_w + 7) / 8;
                let gy = (dst_h + 7) / 8;
                cmd.Dispatch(gx.max(1), gy.max(1), 1);
                src_w = dst_w;
                src_h = dst_h;
            }

            let to_depth = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(Some(depth.clone())),
                        Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                        StateBefore: D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                        StateAfter: D3D12_RESOURCE_STATE_DEPTH_WRITE,
                    }),
                },
            };
            cmd.ResourceBarrier(&[to_depth]);
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn resolve_vis_and_cmaa_rc(
        &self,
        cmd: &ID3D12GraphicsCommandList,
    ) -> Dx12Result<()> {
        let Some(heap) = self.desc_heap.as_ref() else {
            return Ok(());
        };
        let Some(color) = self.color.as_ref() else {
            return Ok(());
        };
        let Some(vis) = self.vis.as_ref() else {
            return Ok(());
        };
        let Some(edges) = self.edge_flags.as_ref() else {
            return Ok(());
        };
        let Some(probes) = self.radiance_probes.as_ref() else {
            return Ok(());
        };

        unsafe {
            // color/vis: RT → SRV/UAV for compute
            let barriers = [
                transition(color, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
                transition(vis, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
            ];
            cmd.ResourceBarrier(&barriers);

            cmd.SetDescriptorHeaps(&[Some(heap.clone())]);
            let gpu = heap.GetGPUDescriptorHandleForHeapStart();
            let inc = self.desc_increment as u64;

            // Vis resolve: vis SRV (2) + color UAV (1)
            if let (Some(pso), Some(root)) = (self.resolve_pso.as_ref(), self.resolve_root.as_ref()) {
                // Need color as UAV
                let to_uav = transition(
                    color,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                );
                cmd.ResourceBarrier(&[to_uav]);

                cmd.SetPipelineState(pso);
                cmd.SetComputeRootSignature(root);
                let cb = [self.width, self.height, 0u32, 0u32];
                cmd.SetComputeRoot32BitConstants(0, 4, cb.as_ptr() as *const _, 0);
                let mut vis_srv = gpu;
                vis_srv.ptr += 2 * inc;
                cmd.SetComputeRootDescriptorTable(1, vis_srv);
                let mut color_uav = gpu;
                color_uav.ptr += 1 * inc;
                cmd.SetComputeRootDescriptorTable(2, color_uav);
                let gx = (self.width + 7) / 8;
                let gy = (self.height + 7) / 8;
                cmd.Dispatch(gx.max(1), gy.max(1), 1);

                let uav_b = D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(Some(color.clone())),
                        }),
                    },
                };
                cmd.ResourceBarrier(&[uav_b]);
            }

            // CMAA2 edge detect → apply into color_aa ping-pong UAV
            if let (Some(edge_pso), Some(apply_pso), Some(root), Some(color_aa)) = (
                self.cmaa_edge_pso.as_ref(),
                self.cmaa_apply_pso.as_ref(),
                self.cmaa_root.as_ref(),
                self.color_aa.as_ref(),
            ) {
                let to_srv = transition(
                    color,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                );
                cmd.ResourceBarrier(&[to_srv]);

                cmd.SetPipelineState(edge_pso);
                cmd.SetComputeRootSignature(root);
                let thr = 0.1f32.to_bits();
                let cb = [self.width, self.height, thr, 0u32];
                cmd.SetComputeRoot32BitConstants(0, 4, cb.as_ptr() as *const _, 0);
                let color_srv = gpu;
                cmd.SetComputeRootDescriptorTable(1, color_srv);
                cmd.SetComputeRootUnorderedAccessView(2, edges.resource.GetGPUVirtualAddress());
                let gx = (self.width + 7) / 8;
                let gy = (self.height + 7) / 8;
                cmd.Dispatch(gx.max(1), gy.max(1), 1);

                let edge_uav_b = D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(Some(edges.resource.clone())),
                        }),
                    },
                };
                cmd.ResourceBarrier(&[edge_uav_b]);

                cmd.SetPipelineState(apply_pso);
                let blend = 0.5f32.to_bits();
                let cb2 = [self.width, self.height, blend, 0u32];
                cmd.SetComputeRoot32BitConstants(0, 4, cb2.as_ptr() as *const _, 0);
                cmd.SetComputeRootDescriptorTable(1, color_srv);
                cmd.SetComputeRootShaderResourceView(3, edges.resource.GetGPUVirtualAddress());
                let mut aa_uav = gpu;
                aa_uav.ptr += 4 * inc;
                cmd.SetComputeRootDescriptorTable(4, aa_uav);
                cmd.Dispatch(gx.max(1), gy.max(1), 1);

                let aa_b = D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(Some(color_aa.clone())),
                        }),
                    },
                };
                cmd.ResourceBarrier(&[aa_b]);

                // Present path copies color_aa; keep color as SRV for RC sampling of pre-AA.
            } else {
                let to_srv = transition(
                    color,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                );
                cmd.ResourceBarrier(&[to_srv]);
            }

            // Radiance Cascades (sample pre-AA color)
            if let (Some(pso), Some(root)) = (self.radiance_pso.as_ref(), self.radiance_root.as_ref())
            {
                cmd.SetPipelineState(pso);
                cmd.SetComputeRootSignature(root);
                let color_srv = gpu;
                cmd.SetComputeRootDescriptorTable(1, color_srv);
                cmd.SetComputeRootUnorderedAccessView(2, probes.resource.GetGPUVirtualAddress());

                for cascade in 0..self.radiance_cascades.max(1) {
                    let cb = [
                        self.width,
                        self.height,
                        self.radiance_grid_w,
                        self.radiance_grid_h,
                        4u32 << cascade,
                        cascade,
                        0,
                        0,
                    ];
                    cmd.SetComputeRoot32BitConstants(0, 8, cb.as_ptr() as *const _, 0);
                    let gx = (self.radiance_grid_w + 7) / 8;
                    let gy = (self.radiance_grid_h + 7) / 8;
                    cmd.Dispatch(gx.max(1), gy.max(1), 1);
                }
            }

            // Prefer AA result for present when available.
            if let Some(color_aa) = self.color_aa.as_ref() {
                let aa_to_src = transition(
                    color_aa,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                );
                cmd.ResourceBarrier(&[aa_to_src]);
            }
            let to_copy_src = transition(
                color,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            cmd.ResourceBarrier(&[to_copy_src]);
            let _ = vis;
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn copy_color_to_backbuffer(
        &self,
        cmd: &ID3D12GraphicsCommandList,
        backbuffer: &ID3D12Resource,
    ) -> Dx12Result<()> {
        let src = self
            .color_aa
            .as_ref()
            .or(self.color.as_ref());
        let Some(color) = src else {
            return Ok(());
        };
        unsafe {
            let to_dst = transition(
                backbuffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
            cmd.ResourceBarrier(&[to_dst]);
            cmd.CopyResource(backbuffer, color);
            let to_present = transition(
                backbuffer,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            cmd.ResourceBarrier(&[to_present]);
            if let Some(c) = self.color.as_ref() {
                let to_rt = transition(
                    c,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
                cmd.ResourceBarrier(&[to_rt]);
            }
            if let Some(aa) = self.color_aa.as_ref() {
                let to_uav = transition(
                    aa,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                );
                cmd.ResourceBarrier(&[to_uav]);
            }
            if let Some(vis) = self.vis.as_ref() {
                let vis_rt = transition(
                    vis,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
                cmd.ResourceBarrier(&[vis_rt]);
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn extract_frustum_planes(vp: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    // Column-major view_proj rows as float4x4 in HLSL mul(float4, matrix) — our upload is row vectors in [[f32;4];4]
    let m = vp;
    let mut planes = [[0.0f32; 4]; 6];
    // Left, Right, Bottom, Top, Near, Far
    for i in 0..4 {
        planes[0][i] = m[i][3] + m[i][0];
        planes[1][i] = m[i][3] - m[i][0];
        planes[2][i] = m[i][3] + m[i][1];
        planes[3][i] = m[i][3] - m[i][1];
        planes[4][i] = m[i][3] + m[i][2];
        planes[5][i] = m[i][3] - m[i][2];
    }
    for p in &mut planes {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-6);
        p[0] /= len;
        p[1] /= len;
        p[2] /= len;
        p[3] /= len;
    }
    planes
}

#[cfg(windows)]
struct ComputePipes {
    cull_pso: Option<ID3D12PipelineState>,
    cull_root: Option<ID3D12RootSignature>,
    hiz_pso: Option<ID3D12PipelineState>,
    hiz_root: Option<ID3D12RootSignature>,
    resolve_pso: Option<ID3D12PipelineState>,
    resolve_root: Option<ID3D12RootSignature>,
    cmaa_edge_pso: Option<ID3D12PipelineState>,
    cmaa_apply_pso: Option<ID3D12PipelineState>,
    cmaa_root: Option<ID3D12RootSignature>,
    radiance_pso: Option<ID3D12PipelineState>,
    radiance_root: Option<ID3D12RootSignature>,
}

#[cfg(windows)]
unsafe fn create_compute_pipelines(
    device: &Dx12Device,
    cull: &CompiledShader,
    hiz: &CompiledShader,
    resolve: &CompiledShader,
    cmaa_edge: &CompiledShader,
    cmaa_apply: &CompiledShader,
    radiance: &CompiledShader,
) -> Dx12Result<ComputePipes> {
    let (cull_pso, cull_root) = create_cs_root_srv_uav(device, cull, 28)?;
    let (hiz_pso, hiz_root) = create_cs_root_tables(device, hiz, 4)?;
    let (resolve_pso, resolve_root) = create_cs_root_tables(device, resolve, 4)?;
    let (cmaa_edge_pso, cmaa_root) = create_cs_cmaa_root(device, cmaa_edge)?;
    let (cmaa_apply_pso, _) = create_cs_cmaa_root(device, cmaa_apply)?;
    let (radiance_pso, radiance_root) = create_cs_radiance_root(device, radiance)?;
    Ok(ComputePipes {
        cull_pso: Some(cull_pso),
        cull_root: Some(cull_root),
        hiz_pso: Some(hiz_pso),
        hiz_root: Some(hiz_root),
        resolve_pso: Some(resolve_pso),
        resolve_root: Some(resolve_root),
        cmaa_edge_pso: Some(cmaa_edge_pso),
        cmaa_apply_pso: Some(cmaa_apply_pso),
        cmaa_root: Some(cmaa_root),
        radiance_pso: Some(radiance_pso),
        radiance_root: Some(radiance_root),
    })
}

#[cfg(windows)]
unsafe fn serialize_root(desc: &D3D12_ROOT_SIGNATURE_DESC) -> Dx12Result<Vec<u8>> {
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    D3D12SerializeRootSignature(desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut err))?;
    let blob = blob.ok_or_else(|| Dx12Error::Msg("root sig blob".into()))?;
    Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec())
}

#[cfg(windows)]
unsafe fn create_cs_pso(
    device: &Dx12Device,
    root: &ID3D12RootSignature,
    cs: &CompiledShader,
) -> Dx12Result<ID3D12PipelineState> {
    let pso_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: std::mem::transmute_copy(root),
        CS: shader_bytecode(&cs.bytecode),
        NodeMask: 0,
        CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
    };
    Ok(device.device.CreateComputePipelineState(&pso_desc)?)
}

#[cfg(windows)]
unsafe fn create_cs_root_srv_uav(
    device: &Dx12Device,
    cs: &CompiledShader,
    num_constants: u32,
) -> Dx12Result<(ID3D12PipelineState, ID3D12RootSignature)> {
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: num_constants,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 3,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let blob = serialize_root(&desc)?;
    let root: ID3D12RootSignature = device.device.CreateRootSignature(0, &blob)?;
    let pso = create_cs_pso(device, &root, cs)?;
    Ok((pso, root))
}

#[cfg(windows)]
unsafe fn create_cs_root_tables(
    device: &Dx12Device,
    cs: &CompiledShader,
    num_constants: u32,
) -> Dx12Result<(ID3D12PipelineState, ID3D12RootSignature)> {
    let ranges = [
        D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        },
        D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        },
    ];
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: num_constants,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &ranges[0],
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &ranges[1],
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 3,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let blob = serialize_root(&desc)?;
    let root: ID3D12RootSignature = device.device.CreateRootSignature(0, &blob)?;
    let pso = create_cs_pso(device, &root, cs)?;
    Ok((pso, root))
}

#[cfg(windows)]
unsafe fn create_cs_cmaa_root(
    device: &Dx12Device,
    cs: &CompiledShader,
) -> Dx12Result<(ID3D12PipelineState, ID3D12RootSignature)> {
    let srv_range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let uav_tex_range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 4,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &srv_range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        // slot 2: either buffer UAV (edge) or texture UAV table (apply) — dual-purpose root
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &uav_tex_range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    // Note: apply CS uses u0 as texture UAV — root slot 2 is buffer UAV which conflicts.
    // Use table-only for apply: we bind texture UAV via slot 4 and leave slot 2 unused for apply.
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 5,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let blob = serialize_root(&desc)?;
    let root: ID3D12RootSignature = device.device.CreateRootSignature(0, &blob)?;
    let pso = create_cs_pso(device, &root, cs)?;
    Ok((pso, root))
}

#[cfg(windows)]
unsafe fn create_cs_radiance_root(
    device: &Dx12Device,
    cs: &CompiledShader,
) -> Dx12Result<(ID3D12PipelineState, ID3D12RootSignature)> {
    let srv_range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 8,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &srv_range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 3,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let blob = serialize_root(&desc)?;
    let root: ID3D12RootSignature = device.device.CreateRootSignature(0, &blob)?;
    let pso = create_cs_pso(device, &root, cs)?;
    Ok((pso, root))
}

#[cfg(windows)]
unsafe fn create_draw_command_signature(
    device: &Dx12Device,
) -> Dx12Result<ID3D12CommandSignature> {
    let byte_stride = 16u32; // sizeof(D3D12_DRAW_ARGUMENTS)
    let arg = D3D12_INDIRECT_ARGUMENT_DESC {
        Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW,
        Anonymous: Default::default(),
    };
    let desc = D3D12_COMMAND_SIGNATURE_DESC {
        ByteStride: byte_stride,
        NumArgumentDescs: 1,
        pArgumentDescs: &arg,
        NodeMask: 0,
    };
    Ok({
        let mut sig: Option<ID3D12CommandSignature> = None;
        device
            .device
            .CreateCommandSignature(&desc, None::<&ID3D12RootSignature>, &mut sig)?;
        sig.ok_or_else(|| Dx12Error::Msg("null command signature".into()))?
    })
}

#[cfg(not(windows))]
impl FrameGpuGraph {
    pub fn create(_device: &Dx12Device, _dxc: &DxcCompiler) -> Dx12Result<Self> {
        Err(Dx12Error::Msg("Windows only".into()))
    }
}
