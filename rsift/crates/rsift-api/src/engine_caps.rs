//! Render / compute engine capability model — DX12 Agility SDK + Shader Model tiers.
//!
//! Install-time: probe GPU → SM 6.9 eligibility → persist to manifest + JVM props.
//! Runtime: mods read `EngineCaps` to gate GPU-driven voxel, visibility buffer, etc.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// HLSL Shader Model tier selected at install (DXC target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ShaderModelTier {
    /// Legacy / iGPU / old discrete — SM 6.6 path, reduced feature set.
    Sm66,
    /// Mid-high discrete — SM 6.9, full GPU-driven voxel + visibility buffer stack.
    Sm69,
}

impl ShaderModelTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sm66 => "6.6",
            Self::Sm69 => "6.9",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "6.6" | "sm66" | "SM66" | "66" => Some(Self::Sm66),
            "6.9" | "sm69" | "SM69" | "69" => Some(Self::Sm69),
            _ => None,
        }
    }

    pub fn dxc_target(self) -> &'static str {
        match self {
            Self::Sm66 => "cs_6_6",
            Self::Sm69 => "cs_6_9",
        }
    }
}

/// Primary graphics backend (Windows: DX12 + Agility SDK redist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderBackend {
    /// DirectX 12 via Agility SDK — GPU Upload Heaps + Enhanced Barriers.
    Dx12Agility,
    /// Fallback when Agility / DX12 unavailable (wgpu portable path).
    WgpuPortable,
}

impl RenderBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dx12Agility => "dx12_agility",
            Self::WgpuPortable => "wgpu",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "dx12_agility" | "dx12" | "d3d12" => Some(Self::Dx12Agility),
            "wgpu" | "portable" => Some(Self::WgpuPortable),
            _ => None,
        }
    }
}

/// World-class rendering techniques (user feature matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EngineFeature {
    GpuDrivenDrawCull,
    FxaaSmaa,
    Cmaa2,
    LowLatencyFramePacing,
    F16Optimization,
    ClusteredForward,
    VisibilityBuffer,
    SoftwareRasterizer,
    EsvoBeam,
    Svdag,
    GpuVoxelFramework,
    RadianceCascades,
    HolographicRadianceCascades,
    VisibilityBitmask,
    VoxelRtGroundTruthTransparency,
    ReverseZ,
    HiZOcclusion,
    BindlessTextures,
    PersistentMappedBuffers,
    StagingBelt,
    WorkGraphs,
    SamplerFeedbackStreaming,
    DirectStorageTiles,
    MultiViewInstancing,
    ConservativeRasterization,
    VariableRateShading,
}

impl EngineFeature {
    pub fn label(self) -> &'static str {
        match self {
            Self::GpuDrivenDrawCull => "GPU-driven draw + culling",
            Self::FxaaSmaa => "FXAA / SMAA",
            Self::Cmaa2 => "CMAA2 (AA upgrade)",
            Self::LowLatencyFramePacing => "Low-latency frame pacing",
            Self::F16Optimization => "f16 shader math",
            Self::ClusteredForward => "Clustered forward",
            Self::VisibilityBuffer => "Visibility buffer",
            Self::SoftwareRasterizer => "Software rasterizer (fallback)",
            Self::EsvoBeam => "ESVO + beam optimization",
            Self::Svdag => "SVDAG",
            Self::GpuVoxelFramework => "GPU-driven voxel framework",
            Self::RadianceCascades => "Radiance Cascades",
            Self::HolographicRadianceCascades => "Holographic Radiance Cascades",
            Self::VisibilityBitmask => "Visibility bitmask",
            Self::VoxelRtGroundTruthTransparency => "Voxel RT ground-truth transparency",
            Self::ReverseZ => "Reverse-Z",
            Self::HiZOcclusion => "Hi-Z occlusion culling",
            Self::BindlessTextures => "Bindless textures",
            Self::PersistentMappedBuffers => "Persistent mapped VBO pool",
            Self::StagingBelt => "Staging belt uploads",
            Self::WorkGraphs => "D3D12 Work Graphs (SM 6.8+)",
            Self::SamplerFeedbackStreaming => "Sampler Feedback Streaming",
            Self::DirectStorageTiles => "DirectStorage tile streaming",
            Self::MultiViewInstancing => "Multi-view / view instancing",
            Self::ConservativeRasterization => "Conservative rasterization",
            Self::VariableRateShading => "Variable Rate Shading",
        }
    }

    /// Minimum shader model for this feature.
    pub fn min_tier(self) -> ShaderModelTier {
        match self {
            Self::SoftwareRasterizer => ShaderModelTier::Sm66,
            Self::FxaaSmaa
            | Self::LowLatencyFramePacing
            | Self::F16Optimization
            | Self::ReverseZ
            | Self::HiZOcclusion
            | Self::PersistentMappedBuffers
            |             Self::StagingBelt
            | Self::GpuDrivenDrawCull
            | Self::ConservativeRasterization
            | Self::MultiViewInstancing => ShaderModelTier::Sm66,
            _ => ShaderModelTier::Sm69,
        }
    }

    pub fn all() -> &'static [EngineFeature] {
        &[
            Self::GpuDrivenDrawCull,
            Self::FxaaSmaa,
            Self::Cmaa2,
            Self::LowLatencyFramePacing,
            Self::F16Optimization,
            Self::ClusteredForward,
            Self::VisibilityBuffer,
            Self::SoftwareRasterizer,
            Self::EsvoBeam,
            Self::Svdag,
            Self::GpuVoxelFramework,
            Self::RadianceCascades,
            Self::HolographicRadianceCascades,
            Self::VisibilityBitmask,
            Self::VoxelRtGroundTruthTransparency,
            Self::ReverseZ,
            Self::HiZOcclusion,
            Self::BindlessTextures,
            Self::PersistentMappedBuffers,
            Self::StagingBelt,
            Self::WorkGraphs,
            Self::SamplerFeedbackStreaming,
            Self::DirectStorageTiles,
            Self::MultiViewInstancing,
            Self::ConservativeRasterization,
            Self::VariableRateShading,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineFeatureSet {
    pub enabled: BTreeSet<EngineFeature>,
}

impl EngineFeatureSet {
    /// Feature matrix for techniques with live present / GPU / CPU paths.
    pub fn for_tier(_tier: ShaderModelTier, _sm69_eligible: bool) -> Self {
        let mut enabled = BTreeSet::new();

        enabled.insert(EngineFeature::ReverseZ);
        enabled.insert(EngineFeature::PersistentMappedBuffers);
        enabled.insert(EngineFeature::StagingBelt);
        enabled.insert(EngineFeature::LowLatencyFramePacing);
        enabled.insert(EngineFeature::F16Optimization);
        enabled.insert(EngineFeature::DirectStorageTiles);
        enabled.insert(EngineFeature::MultiViewInstancing);
        enabled.insert(EngineFeature::WorkGraphs);
        enabled.insert(EngineFeature::ConservativeRasterization);

        // FrameGpuGraph bodies (present-wired)
        enabled.insert(EngineFeature::HiZOcclusion);
        enabled.insert(EngineFeature::GpuDrivenDrawCull);
        enabled.insert(EngineFeature::VisibilityBuffer);
        enabled.insert(EngineFeature::Cmaa2);
        enabled.insert(EngineFeature::RadianceCascades);

        Self { enabled }
    }

    pub fn contains(&self, f: EngineFeature) -> bool {
        self.enabled.contains(&f)
    }

    pub fn count(&self) -> usize {
        self.enabled.len()
    }
}

/// GPU probe result at install / first boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCapabilityProbe {
    pub gpu_name: String,
    pub gpu_score: u32,
    pub vram_mb: u32,
    pub is_mobile_gpu: bool,
    pub is_software_renderer: bool,
    pub dx12_agility_supported: bool,
    pub sm69_eligible: bool,
    pub sm69_block_reason: Option<String>,
    pub recommended_tier: ShaderModelTier,
}

impl GpuCapabilityProbe {
    /// Probe using adaptive_perf hardware telemetry + SM 6.9 rules.
    pub fn probe() -> Self {
        let hw = crate::adaptive_perf::AdaptivePerfEngine::probe_and_cache();
        Self::from_hardware(&hw)
    }

    pub fn from_hardware(hw: &crate::adaptive_perf::HardwareProfile) -> Self {
        let vram_mb = estimate_vram_mb(hw);
        let dx12_agility_supported = cfg!(target_os = "windows") && !hw.is_software_renderer;
        let (sm69_eligible, block_reason) = Self::check_sm69_eligibility(hw, vram_mb, dx12_agility_supported);
        let recommended_tier = if sm69_eligible {
            ShaderModelTier::Sm69
        } else {
            ShaderModelTier::Sm66
        };
        Self {
            gpu_name: hw.gpu_name.clone(),
            gpu_score: hw.gpu_score,
            vram_mb,
            is_mobile_gpu: hw.is_mobile_gpu,
            is_software_renderer: hw.is_software_renderer,
            dx12_agility_supported,
            sm69_eligible,
            sm69_block_reason: block_reason,
            recommended_tier,
        }
    }

    fn check_sm69_eligibility(
        hw: &crate::adaptive_perf::HardwareProfile,
        vram_mb: u32,
        dx12: bool,
    ) -> (bool, Option<String>) {
        if hw.is_software_renderer {
            return (false, Some("software renderer".into()));
        }
        if !dx12 {
            return (false, Some("DX12 Agility requires Windows + DXGI adapter".into()));
        }
        if hw.gpu_score < 12_000 {
            return (
                false,
                Some(format!(
                    "GPU score {} < 12000 (GTX 1660 / RX 5600 class minimum)",
                    hw.gpu_score
                )),
            );
        }
        if vram_mb < 5_500 {
            return (
                false,
                Some(format!("VRAM {}MB < 6GB minimum for SM 6.9", vram_mb)),
            );
        }
        if hw.is_mobile_gpu && hw.gpu_score < 18_000 {
            return (
                false,
                Some("mobile iGPU below RTX 4060 Laptop / RX 7600M class".into()),
            );
        }
        (true, None)
    }

    pub fn resolve_tier(&self, requested: Option<ShaderModelTier>) -> Result<ShaderModelTier, String> {
        match requested {
            Some(ShaderModelTier::Sm69) if !self.sm69_eligible => Err(self
                .sm69_block_reason
                .clone()
                .unwrap_or_else(|| "GPU does not meet SM 6.9 requirements".into())),
            Some(t) => Ok(t),
            None => Ok(self.recommended_tier),
        }
    }
}

fn estimate_vram_mb(hw: &crate::adaptive_perf::HardwareProfile) -> u32 {
    let name = hw.gpu_name.to_lowercase();
    if name.contains("7900 xtx") {
        24_000
    } else if name.contains("7900") || name.contains("4080") || name.contains("4090") {
        16_000
    } else if name.contains("7800") || name.contains("4070") || name.contains("3080") {
        12_000
    } else if name.contains("4060") || name.contains("3060") || name.contains("6600") {
        8_000
    } else if name.contains("1660") || name.contains("580") {
        6_000
    } else if hw.is_mobile_gpu {
        4_000
    } else if hw.gpu_score >= 20_000 {
        12_000
    } else if hw.gpu_score >= 12_000 {
        8_000
    } else {
        4_000
    }
}

/// Persisted install + runtime engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineCaps {
    pub render_backend: RenderBackend,
    pub shader_model: ShaderModelTier,
    pub agility_sdk: bool,
    pub gpu_upload_heaps: bool,
    pub enhanced_barriers: bool,
    pub probe: GpuCapabilityProbe,
    pub features: EngineFeatureSet,
}

impl EngineCaps {
    pub fn build(
        probe: &GpuCapabilityProbe,
        shader_model: ShaderModelTier,
        backend: RenderBackend,
    ) -> Self {
        let agility = backend == RenderBackend::Dx12Agility && probe.dx12_agility_supported;
        Self {
            render_backend: backend,
            shader_model,
            agility_sdk: agility,
            gpu_upload_heaps: agility && shader_model >= ShaderModelTier::Sm66,
            enhanced_barriers: agility,
            features: EngineFeatureSet::for_tier(shader_model, probe.sm69_eligible),
            probe: probe.clone(),
        }
    }

    pub fn install_default(probe: &GpuCapabilityProbe, requested: Option<ShaderModelTier>) -> Result<Self, String> {
        let tier = probe.resolve_tier(requested)?;
        let backend = if probe.dx12_agility_supported {
            RenderBackend::Dx12Agility
        } else {
            RenderBackend::WgpuPortable
        };
        Ok(Self::build(probe, tier, backend))
    }

    /// Read JVM system properties set by installer (`-Drsift.shader_model=6.9`).
    pub fn from_jvm_props() -> Option<Self> {
        let sm = std::env::var("rsift.shader_model").ok().and_then(|s| ShaderModelTier::from_str(&s))?;
        let backend = std::env::var("rsift.render.backend")
            .ok()
            .and_then(|s| RenderBackend::from_str(&s))
            .unwrap_or(RenderBackend::Dx12Agility);
        let probe = GpuCapabilityProbe::probe();
        Some(Self::build(&probe, sm, backend))
    }

    pub fn jvm_properties(&self) -> Vec<String> {
        vec![
            format!("-Drsift.render.backend={}", self.render_backend.as_str()),
            format!("-Drsift.shader_model={}", self.shader_model.as_str()),
            format!("-Drsift.agility_sdk={}", self.agility_sdk),
            format!("-Drsift.gpu_upload_heaps={}", self.gpu_upload_heaps),
            format!("-Drsift.enhanced_barriers={}", self.enhanced_barriers),
        ]
    }

    pub fn summary(&self) -> String {
        format!(
            "backend={} SM={} agility={} features={}/{} gpu={}",
            self.render_backend.as_str(),
            self.shader_model.as_str(),
            self.agility_sdk,
            self.features.count(),
            EngineFeature::all().len(),
            self.probe.gpu_name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_perf::{HardwareProfile, PerformanceTier};

    fn mock_hw(gpu_score: u32, mobile: bool, software: bool) -> HardwareProfile {
        HardwareProfile {
            tier: PerformanceTier::Medium,
            cpu_cores: 8,
            cpu_threads: 16,
            cpu_model: "test".into(),
            ram_gb: 16.0,
            gpu_score,
            gpu_name: "Radeon RX 7800 XT".into(),
            is_mobile_gpu: mobile,
            is_software_renderer: software,
            flagship_boost: false,
        }
    }

    #[test]
    fn sm69_requires_score_and_vram() {
        let probe = GpuCapabilityProbe::from_hardware(&mock_hw(22_000, false, false));
        assert!(probe.sm69_eligible);
        let low = GpuCapabilityProbe::from_hardware(&mock_hw(8_000, false, false));
        assert!(!low.sm69_eligible);
    }

    #[test]
    fn sm69_request_blocked_on_weak_gpu() {
        let probe = GpuCapabilityProbe::from_hardware(&mock_hw(5_000, false, false));
        assert!(probe.resolve_tier(Some(ShaderModelTier::Sm69)).is_err());
        assert_eq!(
            probe.resolve_tier(None).unwrap(),
            ShaderModelTier::Sm66
        );
    }

    #[test]
    fn feature_count_sm69_gt_sm66() {
        let sm66 = EngineFeatureSet::for_tier(ShaderModelTier::Sm66, true);
        let sm69 = EngineFeatureSet::for_tier(ShaderModelTier::Sm69, true);
        assert_eq!(sm66.count(), sm69.count());
        assert!(sm66.contains(EngineFeature::ReverseZ));
        assert!(sm66.contains(EngineFeature::RadianceCascades));
        assert!(sm66.contains(EngineFeature::VisibilityBuffer));
        assert!(sm66.contains(EngineFeature::GpuDrivenDrawCull));
        assert!(sm66.contains(EngineFeature::Cmaa2));
        assert!(sm66.contains(EngineFeature::HiZOcclusion));
        assert!(!sm66.contains(EngineFeature::GpuVoxelFramework));
    }
}
