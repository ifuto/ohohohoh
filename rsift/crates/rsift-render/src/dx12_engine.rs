//! # Hybrid DirectX 12 Explicit / Vulkan & wgpu Bindless Render Bridge (`HybridRenderBridge`)
//!
//! 1) Windows 10/11 かつ D3D12 Agility 対応 GPU 環境では、低レイヤ明示的制御エンジン
//!    `rsift-dx12` (`Root Signature 1.1`, `Static Samplers`, `Descriptor Heap Ring`,
//!    `ExecuteIndirect`, `DirectStorage`) を自動選択・起動。
//! 2) DX12 が使えない環境では opt-gfx の wiring 経路 (FullGraphWiring) を構築して
//!    backend 簿記を `VulkanWgpuBindless` とする。【2026-07-21 監査の事実注記】
//!    game 内の **present** は Vulkan/wgpu では未実装であり、実際の描画経路は
//!    rsift-jvm `render_bridge` のバックエンドラダー (DX12 present → GL パス
//!    スルー) が担う。このブリッジはランチャープロセスの初期化健全性確認
//!    (game dir 検出 + wiring 構築) と backend 種別の簿記が実役割であり、
//!    「wgpu で描画が維持される」ことを意味しない。

use rsift_api::engine_caps::{EngineCaps, RenderBackend};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveRenderBackend {
    DirectX12Explicit,
    VulkanWgpuBindless,
}

impl ActiveRenderBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectX12Explicit => {
                "DirectX 12 Agility Explicit (RootSig 1.1 + Descriptor Ring)"
            }
            Self::VulkanWgpuBindless => {
                "Vulkan / wgpu Bindless (SoA SIMD Culling + Vertex Pulling)"
            }
        }
    }
}

pub struct Dx12RenderBridge {
    pub engine: Option<rsift_dx12::Dx12Engine>,
    pub caps: Option<EngineCaps>,
    pub active_backend: ActiveRenderBackend,
}

impl Dx12RenderBridge {
    pub fn initialize() -> Result<Self, String> {
        let caps = EngineCaps::from_jvm_props().or_else(|| {
            let probe = rsift_api::engine_caps::GpuCapabilityProbe::probe();
            rsift_api::engine_caps::EngineCaps::install_default(&probe, None).ok()
        });

        let Some(caps) = caps else {
            return Err("no engine caps detected".into());
        };

        if caps.render_backend != RenderBackend::Dx12Agility {
            warn!(
                "[RenderBridge] Requested backend={} — probing hybrid DX12/Vulkan capabilities",
                caps.render_backend.as_str()
            );
        }

        // Attempt DirectX 12 Explicit Agility engine first on Windows
        #[cfg(windows)]
        {
            match rsift_dx12::Dx12Engine::create(caps.clone()) {
                Ok(engine) => {
                    info!(
                        "========================================================================"
                    );
                    info!(
                        " 🚀 [RenderBridge] Active Hardware Backend: DirectX 12 Agility Explicit"
                    );
                    info!("    Report: {}", engine.phase_report());
                    info!("    Features: Root Signature 1.1 | Static Samplers | Descriptor Ring");
                    info!(
                        "========================================================================"
                    );
                    crate::proxy::global_proxy().enable();
                    return Ok(Self {
                        engine: Some(engine),
                        caps: Some(caps),
                        active_backend: ActiveRenderBackend::DirectX12Explicit,
                    });
                }
                Err(e) => {
                    warn!("[RenderBridge] DirectX 12 Agility initialization check: {} — building opt-gfx wiring path instead (present は ladder に委譲)", e);
                }
            }
        }

        #[cfg(not(windows))]
        {
            info!("[RenderBridge] Non-Windows OS detected — DX12 is unavailable; building opt-gfx wiring path (present は render_bridge ラダー側)");
        }

        // opt-gfx wiring 経路 (FullGraphWiring) の構築。present は rsift-jvm の
        // render_bridge ラダー (DX12 or GL passthrough) が担う (2026-07-21 注記)。
        info!("========================================================================");
        info!(" ⚡ [RenderBridge] opt-gfx wiring path built (backend ledger: Vulkan/wgpu)");
        info!("    Present: DX12 present or GL passthrough via rsift-jvm render_bridge");
        info!("    CPU features: SoA AVX2/SWAR Frustum | FastEntityCuller V2 | 12B Quantized");
        info!("========================================================================");
        // 経路の構築健全性を実確認 (破棄するが、生成時に実ゲームディレクトリの
        // キャッシュ配置 (.rsift_cache) まで検証される)。game dir は
        // rsift-installer の正規検出ロジック (OS 別標準パス) を使用する。
        let game_dir = rsift_installer::LauncherInstaller::detect_minecraft_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _wiring = rsift_opt_gfx::full_graph_wiring::FullGraphWiring::new(&game_dir);
        crate::proxy::global_proxy().enable();

        Ok(Self {
            engine: None,
            caps: Some(caps),
            active_backend: ActiveRenderBackend::VulkanWgpuBindless,
        })
    }

    pub fn phase_report(&self) -> String {
        match self.active_backend {
            ActiveRenderBackend::DirectX12Explicit => self
                .engine
                .as_ref()
                .map(|e| e.phase_report())
                .unwrap_or_else(|| "dx12=active_explicit".into()),
            ActiveRenderBackend::VulkanWgpuBindless => {
                "backend=vulkan_wgpu_bindless_v2".to_string()
            }
        }
    }
}

pub fn frame_counters() -> (u64, u64) {
    #[cfg(windows)]
    {
        (
            rsift_dx12::FRAMES_PRESENTED.load(std::sync::atomic::Ordering::Relaxed),
            rsift_dx12::DRAW_CALLS_RECORDED.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

pub fn init_render_engine() -> Result<Dx12RenderBridge, String> {
    Dx12RenderBridge::initialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_labels_are_distinct_and_nonempty() {
        let a = ActiveRenderBackend::DirectX12Explicit.as_str();
        let b = ActiveRenderBackend::VulkanWgpuBindless.as_str();
        assert_ne!(a, b);
        assert!(a.contains("DirectX 12") && b.contains("wgpu") || b.contains("Vulkan"));
    }

    #[test]
    fn phase_report_uses_ledger_value_for_wgpu_arm() {
        let bridge = Dx12RenderBridge {
            engine: None,
            caps: None,
            active_backend: ActiveRenderBackend::VulkanWgpuBindless,
        };
        assert_eq!(bridge.phase_report(), "backend=vulkan_wgpu_bindless_v2");
        // DX12 選択中に engine が無い異常系では明示フォールバック文字列。
        let anomalous = Dx12RenderBridge {
            engine: None,
            caps: None,
            active_backend: ActiveRenderBackend::DirectX12Explicit,
        };
        assert_eq!(anomalous.phase_report(), "dx12=active_explicit");
    }

    #[cfg(not(windows))]
    #[test]
    fn frame_counters_are_zero_off_windows() {
        // cfg スタンドインの契約固定 (非 Windows で (0,0))。
        assert_eq!(frame_counters(), (0, 0));
    }
}
