//! Phase 2 — DXC shader compiler (SM 6.6 / 6.8 / 6.9) via dxc.exe / PATH discovery.
//!
//! Low-spec policy: never return fake zeroed DXIL (that crashes the driver).
//! If dxc is missing, return a clear error so callers can fall back to Eco/wgpu paths.

use crate::error::{Dx12Error, Dx12Result};
use rsift_api::engine_caps::ShaderModelTier;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct CompiledShader {
    pub entry: String,
    pub target: String,
    pub bytecode: Vec<u8>,
}

pub struct DxcCompiler {
    pub tier: ShaderModelTier,
}

impl DxcCompiler {
    pub fn new(tier: ShaderModelTier) -> Self {
        Self { tier }
    }

    pub fn target_vs(&self) -> &'static str {
        match self.tier {
            ShaderModelTier::Sm69 => "vs_6_9",
            _ => "vs_6_6",
        }
    }

    pub fn target_ps(&self) -> &'static str {
        match self.tier {
            ShaderModelTier::Sm69 => "ps_6_9",
            _ => "ps_6_6",
        }
    }

    pub fn target_cs(&self) -> &'static str {
        match self.tier {
            ShaderModelTier::Sm69 => "cs_6_9",
            _ => "cs_6_6",
        }
    }

    pub fn target_node(&self) -> &'static str {
        "lib_6_8"
    }

    pub fn target_mesh(&self) -> &'static str {
        "ms_6_8"
    }

    fn find_dxc_exe() -> Option<std::path::PathBuf> {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();

        // Same-dir / PATH
        candidates.push(std::path::PathBuf::from("dxc.exe"));

        // Windows SDK common roots
        let sdk_roots = [
            r"C:\Program Files (x86)\Windows Kits\10\bin",
            r"C:\Program Files\Windows Kits\10\bin",
            r"C:\Program Files (x86)\Microsoft DirectX SDK (June 2010)\Utilities\bin\x64",
        ];
        for root in sdk_roots {
            let root_path = std::path::Path::new(root);
            if let Ok(entries) = std::fs::read_dir(root_path) {
                for entry in entries.flatten() {
                    let dxc = entry.path().join("x64").join("dxc.exe");
                    if dxc.is_file() {
                        candidates.push(dxc);
                    }
                    let dxc32 = entry.path().join("x86").join("dxc.exe");
                    if dxc32.is_file() {
                        candidates.push(dxc32);
                    }
                }
            }
        }

        // Explicit well-known versions
        for ver in [
            "10.0.26100.0",
            "10.0.22621.0",
            "10.0.22000.0",
            "10.0.19041.0",
        ] {
            candidates.push(std::path::PathBuf::from(format!(
                r"C:\Program Files (x86)\Windows Kits\10\bin\{ver}\x64\dxc.exe"
            )));
        }

        // Vulkan SDK ships dxc
        if let Ok(vulk) = std::env::var("VULKAN_SDK") {
            candidates.push(std::path::PathBuf::from(vulk).join("Bin").join("dxc.exe"));
        }

        // Agility / local prebuilt
        candidates.push(std::path::PathBuf::from("windows_binaries").join("dxc.exe"));
        candidates.push(std::path::PathBuf::from("third_party").join("dxc").join("dxc.exe"));

        for c in candidates {
            if c.is_file() {
                return Some(c);
            }
        }

        // `where dxc` on Windows
        #[cfg(windows)]
        {
            if let Ok(out) = std::process::Command::new("where").arg("dxc").output() {
                if out.status.success() {
                    if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                        let p = std::path::PathBuf::from(line.trim());
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }

        None
    }

    #[cfg(windows)]
    pub fn compile_hlsl(
        &self,
        source: &str,
        entry: &str,
        target: &str,
    ) -> Dx12Result<CompiledShader> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let Some(dxc) = Self::find_dxc_exe() else {
            warn!(
                "[DXC] dxc.exe not found — refusing fake bytecode (entry={}). \
                 Install Windows SDK or put dxc on PATH; engine will use Eco/wgpu fallback.",
                entry
            );
            return Err(Dx12Error::Msg(format!(
                "dxc.exe not found (needed for {} / {})",
                entry, target
            )));
        };

        let mut child = Command::new(&dxc)
            .args([
                "-T",
                target,
                "-E",
                entry,
                "-Fo",
                "-", // write DXIL to stdout
                "-Qstrip_reflect",
                "-Qstrip_debug",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Dx12Error::Msg(format!("spawn dxc: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(source.as_bytes());
        }

        let output = child
            .wait_with_output()
            .map_err(|e| Dx12Error::Msg(format!("dxc wait: {}", e)))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(Dx12Error::Msg(format!("DXC failed: {}", err)));
        }

        let bytecode = output.stdout;
        if bytecode.len() < 4 {
            return Err(Dx12Error::Msg(format!(
                "DXC produced empty bytecode for {}",
                entry
            )));
        }

        debug!(
            "[DXC] {} → {} ({} bytes) via {}",
            entry,
            target,
            bytecode.len(),
            dxc.display()
        );
        Ok(CompiledShader {
            entry: entry.into(),
            target: target.into(),
            bytecode,
        })
    }

    #[cfg(not(windows))]
    pub fn compile_hlsl(
        &self,
        _source: &str,
        entry: &str,
        target: &str,
    ) -> Dx12Result<CompiledShader> {
        Err(Dx12Error::Msg("Windows only".into()))
    }

    pub fn compile_embedded_terrain(&self) -> Dx12Result<(CompiledShader, CompiledShader)> {
        const TERRAIN_VS: &str = include_str!("../shaders/terrain_vs.hlsl");
        const TERRAIN_PS: &str = include_str!("../shaders/terrain_ps.hlsl");
        let vs = self.compile_hlsl(TERRAIN_VS, "VsMain", self.target_vs())?;
        let ps = self.compile_hlsl(TERRAIN_PS, "PsMain", self.target_ps())?;
        info!("[DXC] terrain VS/PS compiled for {}", self.target_vs());
        Ok((vs, ps))
    }
}
