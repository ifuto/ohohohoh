//! # Iris Shaders / OptiFine Shader Pack Pipeline
//!
//! Loads OptiFine/Iris-style packs (`gbuffers_*`, `shadow`, `composite`, `final`) and
//! builds a wgpu-oriented pass plan. Full GLSL→WGSL translation of arbitrary packs is
//! out of scope; instead we:
//! 1. Prefer on-disk `.vsh`/`.fsh` when present (stored as source for a future transpiler)
//! 2. Fall back to **Eco WGSL** builtins that are cheap on low-end GPUs
//! 3. Skip shadow / heavy composites on Minimal–Medium tiers

use rsift_api::adaptive_perf::PerformanceTier;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Shader pass type (`gbuffers`, `shadow`, `composite`, `final`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderPassType {
    GBuffersTerrain,
    GBuffersEntities,
    GBuffersWater,
    ShadowMap,
    Composite(u8),
    Final,
}

impl ShaderPassType {
    pub fn pack_stem(self) -> &'static str {
        match self {
            Self::GBuffersTerrain => "gbuffers_terrain",
            Self::GBuffersEntities => "gbuffers_entities",
            Self::GBuffersWater => "gbuffers_water",
            Self::ShadowMap => "shadow",
            Self::Composite(0) => "composite",
            Self::Composite(n) => match n {
                1 => "composite1",
                2 => "composite2",
                _ => "composite",
            },
            Self::Final => "final",
        }
    }

    /// Estimated relative GPU cost (higher = more expensive).
    pub fn cost_weight(self) -> u8 {
        match self {
            Self::GBuffersTerrain => 3,
            Self::GBuffersEntities => 2,
            Self::GBuffersWater => 2,
            Self::ShadowMap => 5,
            Self::Composite(_) => 4,
            Self::Final => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderSourceKind {
    /// Built-in Eco WGSL (always available, low cost).
    EcoWgsl,
    /// Raw GLSL from pack (needs transpilation before GPU use).
    PackGlsl,
}

#[derive(Debug, Clone)]
pub struct LoadedShaderProgram {
    pub name: String,
    pub pass_type: ShaderPassType,
    pub vertex_source: String,
    pub fragment_source: String,
    pub source_kind: ShaderSourceKind,
    pub is_compiled: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IrisUniformBuffer {
    pub model_view_matrix: [f32; 16],
    pub projection_matrix: [f32; 16],
    pub normal_matrix: [f32; 16],
    pub sun_position: [f32; 3],
    pub frame_time_counter: f32,
    pub rain_strength: f32,
    pub aspect_ratio: f32,
    pub near_plane: f32,
    pub far_plane: f32,
}

/// One scheduled draw/fullscreen pass for the frame.
#[derive(Debug, Clone)]
pub struct IrisPassCommand {
    pub pass: ShaderPassType,
    pub name: String,
    pub source_kind: ShaderSourceKind,
}

/// Quality profile controlling which Iris passes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrisQuality {
    /// No shader pack — Sodium/Eco only.
    Off,
    /// Terrain + final only (weak PCs).
    Eco,
    /// + entities/water, no shadow.
    Balanced,
    /// Full deferred-like order including shadow + composite.
    Full,
}

impl IrisQuality {
    pub fn for_tier(tier: PerformanceTier) -> Self {
        match tier {
            PerformanceTier::Minimal | PerformanceTier::Low => Self::Off,
            PerformanceTier::Medium => Self::Eco,
            PerformanceTier::High => Self::Balanced,
        }
    }

    pub fn allows(self, pass: ShaderPassType) -> bool {
        match self {
            Self::Off => false,
            Self::Eco => matches!(
                pass,
                ShaderPassType::GBuffersTerrain | ShaderPassType::Final
            ),
            Self::Balanced => !matches!(
                pass,
                ShaderPassType::ShadowMap | ShaderPassType::Composite(_)
            ),
            Self::Full => true,
        }
    }
}

pub struct IrisShaderEngine {
    pub shaderpack_dir: PathBuf,
    pub active_pack_name: Option<String>,
    pub loaded_programs: HashMap<ShaderPassType, LoadedShaderProgram>,
    pub is_enabled: bool,
    pub quality: IrisQuality,
    /// Last frame's pass plan (for wgpu encoder / debug).
    pub last_frame_plan: Vec<IrisPassCommand>,
}

impl IrisShaderEngine {
    pub fn new(shaderpack_dir: impl Into<PathBuf>) -> Self {
        Self {
            shaderpack_dir: shaderpack_dir.into(),
            active_pack_name: None,
            loaded_programs: HashMap::new(),
            is_enabled: false,
            quality: IrisQuality::Off,
            last_frame_plan: Vec::new(),
        }
    }

    pub fn with_tier(shaderpack_dir: impl Into<PathBuf>, tier: PerformanceTier) -> Self {
        let mut e = Self::new(shaderpack_dir);
        e.quality = IrisQuality::for_tier(tier);
        e
    }

    pub fn discover_shaderpacks(&self) -> Vec<String> {
        let mut packs = Vec::new();
        if !self.shaderpack_dir.exists() {
            let _ = std::fs::create_dir_all(&self.shaderpack_dir);
            return packs;
        }
        if let Ok(entries) = std::fs::read_dir(&self.shaderpack_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() || path.extension().map_or(false, |ext| ext == "zip") {
                    if let Some(name) = path.file_stem() {
                        packs.push(name.to_string_lossy().to_string());
                    }
                }
            }
        }
        packs
    }

    pub fn load_shaderpack(&mut self, pack_name: &str) -> Result<(), String> {
        if self.quality == IrisQuality::Off {
            info!(
                "[Iris] Skipping pack '{}' — quality Off (low-spec / AdaptivePerf)",
                pack_name
            );
            self.is_enabled = false;
            self.active_pack_name = None;
            self.loaded_programs.clear();
            return Ok(());
        }

        info!("================================================================");
        info!(
            " [Iris] Loading shader pack: {} (quality={:?})",
            pack_name, self.quality
        );
        info!("================================================================");

        self.loaded_programs.clear();
        self.active_pack_name = Some(pack_name.to_string());
        self.is_enabled = true;

        let pack_root = self.resolve_pack_root(pack_name);
        let passes = [
            ShaderPassType::GBuffersTerrain,
            ShaderPassType::GBuffersEntities,
            ShaderPassType::GBuffersWater,
            ShaderPassType::ShadowMap,
            ShaderPassType::Composite(0),
            ShaderPassType::Final,
        ];

        for pass in passes {
            if !self.quality.allows(pass) {
                continue;
            }
            self.register_program(pass, pack_root.as_deref());
        }

        info!(
            "[Iris] Ready: {} passes for [{}]",
            self.loaded_programs.len(),
            pack_name
        );
        Ok(())
    }

    fn resolve_pack_root(&self, pack_name: &str) -> Option<PathBuf> {
        let dir = self.shaderpack_dir.join(pack_name);
        if dir.is_dir() {
            // OptiFine layout: shaders/ inside pack folder
            let shaders = dir.join("shaders");
            return Some(if shaders.is_dir() { shaders } else { dir });
        }
        // Zip packs (実実装): shaders/*.fsh|vsh|glsl|properties|lang を実展開して
        // キャッシュルートを返す。展開物は zip の (mtime,size) スタンプで整合確認。
        if dir.is_file() && pack_name.ends_with(".zip") {
            let stem = pack_name.trim_end_matches(".zip");
            let cache = self.shaderpack_dir.join(".rsift_extracted").join(stem);
            if let Ok(meta) = std::fs::metadata(&dir) {
                let stamp = format!(
                    "{}:{}",
                    meta.len(),
                    meta.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let stamp_path = cache.join(".stamp");
                if std::fs::read_to_string(&stamp_path).ok().as_deref() == Some(stamp.as_str()) {
                    // 展開済キャッシュが最新 → 再利用。
                    let shaders = cache.join("shaders");
                    return Some(if shaders.is_dir() { shaders } else { cache });
                }
                match extract_zip_shaders(&dir, &cache) {
                    Ok(n) => {
                        let _ = std::fs::write(&stamp_path, stamp);
                        info!(
                            "[Iris] zip pack extracted: {} entries → {}",
                            n,
                            cache.display()
                        );
                        let shaders = cache.join("shaders");
                        return Some(if shaders.is_dir() { shaders } else { cache });
                    }
                    Err(e) => {
                        warn!("[Iris] zip pack extraction failed ({}): {}", pack_name, e);
                    }
                }
            }
        }
        None
    }

    fn register_program(&mut self, pass_type: ShaderPassType, pack_shaders: Option<&Path>) {
        let name = pass_type.pack_stem().to_string();
        if let Some(root) = pack_shaders {
            let vsh = root.join(format!("{}.vsh", name));
            let fsh = root.join(format!("{}.fsh", name));
            if vsh.is_file() && fsh.is_file() {
                warn!(
                    "[Iris] Pack GLSL found for {} but GLSL→HLSL/WGSL transpile is not wired — \
                     refusing uncompiled GPU program (falling back to Eco WGSL)",
                    name
                );
                // Do not register is_compiled:false — fall through to Eco.
            }
        }

        let (vs, fs) = eco_wgsl_for(pass_type);
        self.loaded_programs.insert(
            pass_type,
            LoadedShaderProgram {
                name,
                pass_type,
                vertex_source: vs.to_string(),
                fragment_source: fs.to_string(),
                source_kind: ShaderSourceKind::EcoWgsl,
                is_compiled: true,
            },
        );
    }

    pub fn disable_shaders(&mut self) {
        if self.is_enabled {
            info!("[Iris] Disabled → Eco/Sodium path");
            self.is_enabled = false;
            self.active_pack_name = None;
            self.last_frame_plan.clear();
        }
    }

    /// Build the per-frame pass plan (shadow → gbuffers → composite → final).
    pub fn plan_frame(&self) -> Vec<IrisPassCommand> {
        if !self.is_enabled {
            return Vec::new();
        }
        let order = [
            ShaderPassType::ShadowMap,
            ShaderPassType::GBuffersTerrain,
            ShaderPassType::GBuffersEntities,
            ShaderPassType::GBuffersWater,
            ShaderPassType::Composite(0),
            ShaderPassType::Final,
        ];
        order
            .into_iter()
            .filter_map(|pass| {
                let prog = self.loaded_programs.get(&pass)?;
                Some(IrisPassCommand {
                    pass,
                    name: prog.name.clone(),
                    source_kind: prog.source_kind,
                })
            })
            .collect()
    }

    /// Record the frame plan. Callers feed `last_frame_plan` into a wgpu encoder.
    ///
    /// Only Eco WGSL passes are GPU-ready today; pack GLSL is kept loaded for
    /// a future transpiler but skipped at draw time (avoids driver stalls).
    pub fn dispatch_frame_passes(&mut self, uniforms: &IrisUniformBuffer) -> &[IrisPassCommand] {
        if !self.is_enabled {
            self.last_frame_plan.clear();
            return &self.last_frame_plan;
        }
        let _ = uniforms;
        self.last_frame_plan = self
            .plan_frame()
            .into_iter()
            .filter(|c| c.source_kind == ShaderSourceKind::EcoWgsl)
            .collect();
        if self.last_frame_plan.is_empty() {
            warn!("[Iris] No Eco WGSL passes; injecting terrain+final");
            for pass in [ShaderPassType::GBuffersTerrain, ShaderPassType::Final] {
                let (vs, fs) = eco_wgsl_for(pass);
                self.loaded_programs.insert(
                    pass,
                    LoadedShaderProgram {
                        name: pass.pack_stem().into(),
                        pass_type: pass,
                        vertex_source: vs.into(),
                        fragment_source: fs.into(),
                        source_kind: ShaderSourceKind::EcoWgsl,
                        is_compiled: true,
                    },
                );
            }
            self.last_frame_plan = self
                .plan_frame()
                .into_iter()
                .filter(|c| c.source_kind == ShaderSourceKind::EcoWgsl)
                .collect();
        }
        debug!(
            "[Iris] frame plan: {} passes (cost≈{})",
            self.last_frame_plan.len(),
            self.last_frame_plan
                .iter()
                .map(|c| c.pass.cost_weight() as u32)
                .sum::<u32>()
        );
        &self.last_frame_plan
    }
}

/// Minimal fullscreen / mesh WGSL used when packs are missing or too heavy.
fn eco_wgsl_for(pass: ShaderPassType) -> (&'static str, &'static str) {
    match pass {
        ShaderPassType::GBuffersTerrain | ShaderPassType::GBuffersEntities => {
            (ECO_VS_MESH, ECO_FS_TERRAIN)
        }
        ShaderPassType::GBuffersWater => (ECO_VS_MESH, ECO_FS_WATER),
        ShaderPassType::ShadowMap => (ECO_VS_MESH, ECO_FS_SHADOW),
        ShaderPassType::Composite(_) => (ECO_VS_FULLSCREEN, ECO_FS_COMPOSITE),
        ShaderPassType::Final => (ECO_VS_FULLSCREEN, ECO_FS_FINAL),
    }
}

const ECO_VS_MESH: &str = r#"
struct Uniforms {
    model_view: mat4x4<f32>,
    projection: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex
fn vs_main(in: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = u.projection * u.model_view * vec4<f32>(in.position, 1.0);
    o.uv = in.uv;
    o.color = in.color;
    return o;
}
"#;

const ECO_FS_TERRAIN: &str = r#"
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct FsIn {
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@fragment
fn fs_main(in: FsIn) -> @location(0) vec4<f32> {
    let albedo = textureSample(tex, samp, in.uv) * in.color;
    // Cheap Lambert-ish shade without shadow maps.
    let shade = 0.65 + 0.35 * in.color.g;
    return vec4<f32>(albedo.rgb * shade, albedo.a);
}
"#;

const ECO_FS_WATER: &str = r#"
struct FsIn {
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@fragment
fn fs_main(in: FsIn) -> @location(0) vec4<f32> {
    let c = mix(vec3<f32>(0.15, 0.35, 0.55), in.color.rgb, 0.4);
    return vec4<f32>(c, 0.55);
}
"#;

const ECO_FS_SHADOW: &str = r#"
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#;

const ECO_VS_FULLSCREEN: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var o: VsOut;
    o.clip = vec4<f32>(pos[vid], 0.0, 1.0);
    o.uv = pos[vid] * 0.5 + vec2<f32>(0.5, 0.5);
    return o;
}
"#;

const ECO_FS_COMPOSITE: &str = r#"
@group(0) @binding(0) var color_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // Identity composite — no bloom/DoF on Eco path.
    return textureSample(color_tex, samp, uv);
}
"#;

const ECO_FS_FINAL: &str = r#"
@group(0) @binding(0) var color_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    var c = textureSample(color_tex, samp, uv).rgb;
    // Mild filmic tonemap (cheap, no LUTs).
    c = c * (1.0 + c * 0.2) / (1.0 + c);
    return vec4<f32>(c, 1.0);
}
"#;

// ============================================================
// ZIP shaderpack real extraction (stored + deflate).
// EOCD → Central Directory → Local Header の実走査。Zip64 は非対応
// (エラーを返し Eco builtins へ安全にフォールバック)。展開制限:
// 1 ファイル 16 MiB / 合計 64 MiB / 4096 エントリ ("zip bomb" 抑止)。
// ============================================================

fn read_u16(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}
fn read_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

/// zip のエントリを `dest` 配下に安全パス検証つきで実展開。展開したファイル数を返す。
pub fn extract_zip_shaders(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    use std::io::Read;
    let data = std::fs::read(zip_path).map_err(|e| format!("read: {}", e))?;
    if data.len() < 22 {
        return Err("not a zip (too small)".into());
    }
    // EOCD 探索 (末尾 64 KiB)。
    let scan_start = data.len().saturating_sub(66 * 1024 + 22);
    let mut eocd = None;
    for i in (scan_start..=data.len() - 22).rev() {
        if read_u32(&data, i) == 0x0605_4B50 {
            eocd = Some(i);
            break;
        }
    }
    let eocd = eocd.ok_or("EOCD not found")?;
    let entries = read_u16(&data, eocd + 10) as usize;
    if entries == 0xFFFF {
        return Err("Zip64 unsupported".into());
    }
    let cd_off = read_u32(&data, eocd + 16) as usize;
    let mut pos = cd_off;
    let mut extracted = 0usize;
    let mut total_bytes = 0u64;
    for _ in 0..entries.min(4096) {
        if pos + 46 > data.len() || read_u32(&data, pos) != 0x0201_4B50 {
            break;
        }
        let method = read_u16(&data, pos + 10);
        let comp_size = read_u32(&data, pos + 20) as usize;
        let uncomp_size = read_u32(&data, pos + 24) as usize;
        let nlen = read_u16(&data, pos + 28) as usize;
        let xlen = read_u16(&data, pos + 30) as usize;
        let clen = read_u16(&data, pos + 32) as usize;
        let local_off = read_u32(&data, pos + 42) as usize;
        if pos + 46 + nlen > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[pos + 46..pos + 46 + nlen]).replace('\\', "/");
        let next = pos + 46 + nlen + xlen + clen;
        pos = next;
        // パス検証: 絶対パス/親参照/ドライブを拒否。
        if name.starts_with('/')
            || name.contains(':')
            || name.split('/').any(|seg| seg == "..")
            || name.is_empty()
        {
            continue;
        }
        if name.ends_with('/') {
            continue; // ディレクトリ
        }
        if uncomp_size > 16 * 1024 * 1024 || total_bytes + uncomp_size as u64 > 64 * 1024 * 1024 {
            return Err("extraction size cap exceeded (zip bomb guard)".into());
        }
        if local_off + 30 > data.len() || read_u32(&data, local_off) != 0x0403_4B50 {
            continue;
        }
        let lnlen = read_u16(&data, local_off + 26) as usize;
        let lxlen = read_u16(&data, local_off + 28) as usize;
        let data_off = local_off + 30 + lnlen + lxlen;
        if data_off + comp_size > data.len() {
            continue;
        }
        let comp = &data[data_off..data_off + comp_size];
        let content: Vec<u8> = match method {
            0 => comp.to_vec(),
            8 => {
                let mut dec = flate2::read::DeflateDecoder::new(comp);
                let mut out = Vec::with_capacity(uncomp_size.min(64 * 1024));
                dec.read_to_end(&mut out)
                    .map_err(|e| format!("deflate {}: {}", name, e))?;
                out
            }
            other => return Err(format!("unsupported method {}: {}", other, name)),
        };
        if content.len() != uncomp_size {
            return Err(format!("size mismatch: {}", name));
        }
        let out_path = dest.join(&name);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }
        std::fs::write(&out_path, &content).map_err(|e| format!("write: {}", e))?;
        total_bytes += uncomp_size as u64;
        extracted += 1;
    }
    if extracted == 0 {
        return Err("no entries extracted".into());
    }
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_tier_skips_pack() {
        let mut e = IrisShaderEngine::with_tier("./shaderpacks", PerformanceTier::Low);
        e.load_shaderpack("test").unwrap();
        assert!(!e.is_enabled);
        assert!(e
            .dispatch_frame_passes(&IrisUniformBuffer {
                model_view_matrix: [0.0; 16],
                projection_matrix: [0.0; 16],
                normal_matrix: [0.0; 16],
                sun_position: [0.0; 3],
                frame_time_counter: 0.0,
                rain_strength: 0.0,
                aspect_ratio: 1.0,
                near_plane: 0.05,
                far_plane: 1000.0,
            })
            .is_empty());
    }

    #[test]
    fn eco_loads_terrain_and_final() {
        let mut e = IrisShaderEngine::with_tier("./shaderpacks", PerformanceTier::Medium);
        e.load_shaderpack("eco").unwrap();
        assert!(e.is_enabled);
        assert!(e
            .loaded_programs
            .contains_key(&ShaderPassType::GBuffersTerrain));
        assert!(e.loaded_programs.contains_key(&ShaderPassType::Final));
        assert!(!e.loaded_programs.contains_key(&ShaderPassType::ShadowMap));
    }
}
