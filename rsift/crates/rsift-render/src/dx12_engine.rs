//! DirectX 12 Agility — engine creation (per-thread / mod-init ownership).

use rsift_api::engine_caps::{EngineCaps, RenderBackend};
use tracing::{info, warn};

pub struct Dx12RenderBridge {
    #[cfg(windows)]
    pub engine: Option<rsift_dx12::Dx12Engine>,
    pub caps: Option<EngineCaps>,
}

impl Dx12RenderBridge {
    pub fn initialize() -> Result<Self, String> {
        let caps = EngineCaps::from_jvm_props().or_else(|| {
            let probe = rsift_api::engine_caps::GpuCapabilityProbe::probe();
            rsift_api::engine_caps::EngineCaps::install_default(&probe, None).ok()
        });

        let Some(caps) = caps else {
            return Err("no engine caps".into());
        };

        if caps.render_backend != RenderBackend::Dx12Agility {
            warn!(
                "[Dx12Bridge] backend={} — using DX12 Agility",
                caps.render_backend.as_str()
            );
        }

        #[cfg(windows)]
        {
            let engine = rsift_dx12::Dx12Engine::create(caps.clone())
                .map_err(|e| format!("Dx12Engine::create failed: {}", e))?;
            info!("[Dx12Bridge] {}", engine.phase_report());
            crate::proxy::global_proxy().enable();
            return Ok(Self {
                engine: Some(engine),
                caps: Some(caps),
            });
        }

        #[cfg(not(windows))]
        Err("DirectX 12 requires Windows".into())
    }

    #[cfg(windows)]
    pub fn phase_report(&self) -> String {
        self.engine
            .as_ref()
            .map(|e| e.phase_report())
            .unwrap_or_else(|| "dx12=not_initialized".into())
    }

    #[cfg(not(windows))]
    pub fn phase_report(&self) -> String {
        "dx12=unsupported".into()
    }
}

pub fn frame_counters() -> (u64, u64) {
    (
        rsift_dx12::FRAMES_PRESENTED.load(std::sync::atomic::Ordering::Relaxed),
        rsift_dx12::DRAW_CALLS_RECORDED.load(std::sync::atomic::Ordering::Relaxed),
    )
}

pub fn init_render_engine() -> Result<Dx12RenderBridge, String> {
    Dx12RenderBridge::initialize()
}
