//! # Rsift Official Minecraft Launcher Auto-Installer
//!
//! versions/rsift-loader-{mc_version}_{build_version}/ に JSON・JAR・mods を配置し、
//! 起動構成へ正確に参照を登録する。

use serde_json::{json, Value};
use rsift_api::engine_caps::{EngineCaps, GpuCapabilityProbe, ShaderModelTier};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{error, info, warn};

pub mod embedded_payloads;
use embedded_payloads::EmbeddedPayloads;

pub const BUILD_VERSION: &str = "v1.0.1_perf1";

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// `None` = auto (SM 6.9 if GPU eligible, else 6.6).
    pub shader_model: Option<ShaderModelTier>,
}

#[derive(Debug, Clone)]
pub struct InstallResult {
    pub version_id: String,
    pub version_dir: PathBuf,
    pub version_json: PathBuf,
    pub version_jar: PathBuf,
    pub inherits_from: String,
    pub agent_path: Option<PathBuf>,
    pub agent_loaded: bool,
    pub mods_deployed: Vec<String>,
    pub profile_name: String,
    pub profile_java_args: String,
    pub engine_caps: EngineCaps,
}

pub struct LauncherInstaller {
    pub minecraft_dir: PathBuf,
    pub target_version: String,
    pub build_version: String,
}

impl LauncherInstaller {
    pub fn new() -> Result<Self, String> {
        let mc_dir = Self::detect_minecraft_dir().ok_or_else(|| {
            "Could not automatically detect official Minecraft directory (.minecraft)".to_string()
        })?;
        Ok(Self {
            minecraft_dir: mc_dir,
            target_version: "1.21.11".to_string(),
            build_version: BUILD_VERSION.to_string(),
        })
    }

    pub fn detect_minecraft_dir() -> Option<PathBuf> {
        if cfg!(target_os = "windows") {
            std::env::var("APPDATA").ok().map(|a| Path::new(&a).join(".minecraft"))
        } else if cfg!(target_os = "macos") {
            std::env::var("HOME").ok().map(|h| Path::new(&h).join("Library/Application Support/minecraft"))
        } else {
            std::env::var("HOME").ok().map(|h| Path::new(&h).join(".minecraft"))
        }
    }

    /// `rsift-loader-1.21.11_v1.0.0` — 既存時は `(1)`, `(2)` …
    pub fn resolve_version_id(&self) -> String {
        Self::resolve_version_id_for(&self.minecraft_dir, &self.target_version, &self.build_version)
    }

    pub fn resolve_version_id_for(mc_dir: &Path, game_version: &str, build_version: &str) -> String {
        let base = format!("rsift-loader-{}_{}", game_version, build_version);
        let versions_dir = mc_dir.join("versions");
        if !versions_dir.join(&base).exists() {
            return base;
        }
        for n in 1..=99 {
            let candidate = format!("{}_{}", base, n);
            if !versions_dir.join(&candidate).exists() {
                return candidate;
            }
        }
        format!("{}_{}", base, 99)
    }

    pub fn install(&self, rsift_install_path: &Path) -> Result<InstallResult, String> {
        self.install_with_options(rsift_install_path, InstallOptions::default())
    }

    pub fn install_with_options(
        &self,
        rsift_install_path: &Path,
        options: InstallOptions,
    ) -> Result<InstallResult, String> {
        let probe = GpuCapabilityProbe::probe();
        let engine_caps = EngineCaps::install_default(&probe, options.shader_model)?;
        info!("==========================================================================");
        info!("  Rsift Auto-Installer | build={}", self.build_version);
        info!("  GPU: {} (score={} VRAM~{}MB)", probe.gpu_name, probe.gpu_score, probe.vram_mb);
        info!(
            "  Shader Model: {} | agility={} | features={}/{}",
            engine_caps.shader_model.as_str(),
            engine_caps.agility_sdk,
            engine_caps.features.count(),
            rsift_api::engine_caps::EngineFeature::all().len()
        );
        if !probe.sm69_eligible {
            if let Some(ref reason) = probe.sm69_block_reason {
                info!("  SM 6.9 unavailable: {}", reason);
            }
        }
        if options.shader_model == Some(ShaderModelTier::Sm69) && !probe.sm69_eligible {
            return Err(probe
                .sm69_block_reason
                .unwrap_or_else(|| "GPU does not meet SM 6.9 minimum spec".into()));
        }
        info!("  Minecraft dir: {:?}", self.minecraft_dir);
        info!("  Install root:  {:?}", rsift_install_path);
        info!("  Close the Minecraft Launcher before installing (it overwrites profiles on exit).");
        info!("==========================================================================");

        if !self.minecraft_dir.exists() {
            fs::create_dir_all(&self.minecraft_dir).map_err(|e| e.to_string())?;
        }

        let version_id = self.resolve_version_id();
        let inherits_from = self.target_version.clone();
        let version_dir = self.minecraft_dir.join("versions").join(&version_id);
        fs::create_dir_all(&version_dir).map_err(|e| format!("mkdir {:?}: {}", version_dir, e))?;

        info!("[Install] version_id      = {}", version_id);
        info!("[Install] version_dir     = {:?}", version_dir);
        info!("[Install] inheritsFrom    = {}", inherits_from);

        let abs_install = rsift_install_path
            .canonicalize()
            .unwrap_or_else(|_| rsift_install_path.to_path_buf());

        let agent_path = Self::find_agent_dll(&abs_install);
        let agent_for_launch = if let Some(ref ap) = agent_path {
            let dest = version_dir.join(
                ap.file_name()
                    .ok_or_else(|| "agent dll has no filename".to_string())?,
            );
            fs::copy(ap, &dest).map_err(|e| format!("copy agent dll: {}", e))?;
            info!("[Install] native bridge    = {:?} → {:?}", ap, dest);
            Some(dest)
        } else {
            match EmbeddedPayloads::deploy_rsift_jvm(&version_dir) {
                Ok(dest) => {
                    info!("[Install] native bridge    = self-extracted to {:?}", dest);
                    Some(dest)
                }
                Err(e) => {
                    warn!(
                        "[Install] rsift_jvm.dll    = NOT FOUND — native hooks disabled ({})",
                        e
                    );
                    warn!("[Install]   Searched under: {:?}", abs_install);
                    None
                }
            }
        };
        let agent_loaded = agent_for_launch.is_some();

        let mut jvm_args: Vec<String> = vec![
            "-Drsift.loader.enabled=true".into(),
            "-Drsift.native.injection=true".into(),
            format!("-Drsift.version={}", self.build_version),
            format!("-Drsift.native.path={}", abs_install.display()),
            format!("-Drsift.version.id={}", version_id),
        ];
        jvm_args.extend(engine_caps.jvm_properties());

        self.write_agent_opts(&version_dir, &version_id, &abs_install)?;

        let bootstrap_jar = self.deploy_bootstrap_jar(rsift_install_path, &version_dir)?;

        if let Some(ref ap) = agent_for_launch {
            jvm_args.insert(0, format!("-agentpath:{}", ap.display()));
            info!("[Install] launch mode     = agentpath-only (stable — no -javaagent)");
            info!("[Install] agentpath       = {:?}", ap);
        } else if bootstrap_jar.is_none() {
            warn!("[Install] rsift-bootstrap.jar NOT FOUND — run bootstrap\\build.bat after installing JDK");
        }
        let _ = bootstrap_jar;

        let profile_java_args = Self::build_profile_java_args(&jvm_args);
        let version_json_jvm = Self::version_json_jvm_args(&jvm_args);

        let version_json_path = version_dir.join(format!("{}.json", version_id));
        let version_json_content = json!({
            "id": version_id,
            "inheritsFrom": inherits_from,
            "type": "release",
            "time": "2026-07-05T00:00:00+00:00",
            "releaseTime": "2026-07-05T00:00:00+00:00",
            "mainClass": "net.minecraft.client.main.Main",
            "arguments": { "jvm": version_json_jvm }
        });
        fs::write(&version_json_path, serde_json::to_string_pretty(&version_json_content).unwrap())
            .map_err(|e| format!("write version json: {}", e))?;
        info!("[Install] version_json    = {:?}", version_json_path);

        let version_jar_path = version_dir.join(format!("{}.jar", version_id));
        let vanilla_jar = self
            .minecraft_dir
            .join("versions")
            .join(&inherits_from)
            .join(format!("{}.jar", inherits_from));

        info!("[Install] vanilla_jar ref = {:?}", vanilla_jar);
        if !vanilla_jar.exists() {
            error!("[Install] Vanilla {}.jar NOT FOUND — install vanilla from official launcher first", inherits_from);
            return Err(format!(
                "Required vanilla jar missing: {:?}.\n\
                 1) Open the official Minecraft Launcher\n\
                 2) Install / play vanilla {} once (downloads the client jar)\n\
                 3) Close the launcher completely, then re-run this installer",
                vanilla_jar, inherits_from
            ));
        }
        // Prefer hardlink (saves disk); fall back to copy for cross-volume installs.
        let _ = fs::remove_file(&version_jar_path);
        let linked = std::fs::hard_link(&vanilla_jar, &version_jar_path).is_ok();
        if !linked {
            fs::copy(&vanilla_jar, &version_jar_path)
                .map_err(|e| format!("copy vanilla jar: {}", e))?;
        }
        let jar_size = fs::metadata(&version_jar_path).map(|m| m.len()).unwrap_or(0);
        info!(
            "[Install] version_jar     = {:?} ({} bytes, {})",
            version_jar_path,
            jar_size,
            if linked { "hardlink" } else { "copied" }
        );
        if jar_size < 1_000_000 {
            return Err(format!("version jar too small ({} bytes) — copy/link failed?", jar_size));
        }

        // Official launcher loads mods from gameDir/mods (= .minecraft/mods).
        let mods_deployed = self.deploy_official_dll_mods(rsift_install_path, &version_dir)?;
        self.deploy_agility_sdk(rsift_install_path, &version_dir)?;

        let profiles_path = self.minecraft_dir.join("launcher_profiles.json");
        // gameDir MUST be .minecraft so assets/saves/mods resolve like vanilla.
        let profile_name = self.register_profile(
            &profiles_path,
            &version_id,
            &self.minecraft_dir,
            &profile_java_args,
        )?;

        let repaired = Self::repair_profiles(&self.minecraft_dir).unwrap_or(0);
        if repaired > 0 {
            info!("[Install] repaired {} legacy Rsift profile(s) → agentpath-only", repaired);
        }

        let _ = self.register_to_system_path(rsift_install_path);
        self.write_install_manifest(&version_dir, &InstallResult {
            version_id: version_id.clone(),
            version_dir: version_dir.clone(),
            version_json: version_json_path.clone(),
            version_jar: version_jar_path.clone(),
            inherits_from: inherits_from.clone(),
            agent_path: agent_for_launch.clone(),
            agent_loaded,
            mods_deployed: mods_deployed.clone(),
            profile_name: profile_name.clone(),
            profile_java_args: profile_java_args.clone(),
            engine_caps: engine_caps.clone(),
        })?;

        info!("--------------------------------------------------------------------------");
        info!(" SUCCESS — Rsift registered for the OFFICIAL Minecraft Launcher");
        info!("   Profile name:    {}", profile_name);
        info!("   lastVersionId:   {}", version_id);
        info!("   gameDir:         {:?}", self.minecraft_dir);
        info!("   inheritsFrom:    {}", inherits_from);
        info!("   Launch mode:     agentpath (profile javaArgs)");
        info!("   Native bridge:   {}", if agent_loaded { "YES" } else { "NO (vanilla fallback)" });
        info!("   Mods deployed:   {:?}", mods_deployed);
        info!("--------------------------------------------------------------------------");
        info!(" HOW TO PLAY:");
        info!("   1. Close this installer");
        info!("   2. Open the official Minecraft Launcher");
        info!("   3. Select installation \"{}\"", profile_name);
        info!("   4. Press Play");
        info!(" Verify: rsift-bootstrap.log should contain Agent_OnLoad");
        info!("==========================================================================");

        Ok(InstallResult {
            version_id,
            version_dir,
            version_json: version_json_path,
            version_jar: version_jar_path,
            inherits_from,
            agent_path: agent_for_launch,
            agent_loaded,
            mods_deployed,
            profile_name,
            profile_java_args,
            engine_caps,
        })
    }

    fn find_agent_dll(install_root: &Path) -> Option<PathBuf> {
        let names = if cfg!(target_os = "windows") {
            vec!["rsift_jvm.dll", "rsift_jvm.dll"]
        } else {
            vec!["librsift_jvm.so", "librsift_jvm.dylib"]
        };
        let bases = [
            install_root.join("target/release"),
            install_root.join("target/debug"),
            install_root.to_path_buf(),
        ];
        for base in &bases {
            for name in &names {
                let p = base.join(name);
                if p.exists() {
                    return p.canonicalize().ok().or(Some(p));
                }
            }
        }
        None
    }

    fn build_profile_java_args(jvm_args: &[String]) -> String {
        // Profile carries the full Rsift JVM arg string (launcher appends this).
        // Keep memory flags here as well for parity with older working profiles.
        let mut args: Vec<String> = jvm_args.to_vec();
        if !args.iter().any(|a| a.starts_with("-Xms")) {
            args.push("-Xms4G".into());
        }
        if !args.iter().any(|a| a.starts_with("-Xmx")) {
            args.push("-Xmx8G".into());
        }
        if !args.iter().any(|a| a.contains("UseZGC")) {
            args.push("-XX:+UnlockExperimentalVMOptions".into());
            args.push("-XX:+UseZGC".into());
            args.push("-XX:+ZGenerational".into());
        }
        args.join(" ")
    }

    /// Canonical agentpath-only javaArgs for a version directory (single source of truth).
    pub fn canonical_java_args(version_dir: &Path, version_id: &str, install_root: &Path) -> String {
        let dll = version_dir.join("rsift_jvm.dll");
        let jvm_args = vec![
            format!("-agentpath:{}", dll.display()),
            "-Drsift.loader.enabled=true".into(),
            "-Drsift.native.injection=true".into(),
            format!("-Drsift.version={}", BUILD_VERSION),
            format!("-Drsift.native.path={}", install_root.display()),
            format!("-Drsift.version.id={}", version_id),
        ];
        Self::build_profile_java_args(&jvm_args)
    }

    /// Version JSON must not carry -agentpath/-javaagent (launcher also applies profile javaArgs).
    fn version_json_jvm_args(jvm_args: &[String]) -> Vec<String> {
        jvm_args
            .iter()
            .filter(|a| a.starts_with("-Drsift."))
            .cloned()
            .collect()
    }

    fn is_launch_agent_flag(arg: &str) -> bool {
        arg.starts_with("-agentpath:") || arg.starts_with("-javaagent:")
    }

    /// Strip legacy `-javaagent:` from profile javaArgs (causes exit 1 with agentpath on some JVMs).
    fn strip_javaagent_from_args(java_args: &str) -> String {
        java_args
            .split_whitespace()
            .filter(|a| !a.starts_with("-javaagent:"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn unique_profile_name(existing: &[String], base: &str) -> String {
        for n in 1..=999 {
            let candidate = format!("{} ({})", base, n);
            if !existing.contains(&candidate) {
                return candidate;
            }
        }
        format!("{} (999)", base)
    }

    /// Repair launcher_profiles.json — rewrite every Rsift profile to agentpath-only for its lastVersionId.
    pub fn repair_profiles(minecraft_dir: &Path) -> Result<usize, String> {
        let path = minecraft_dir.join("launcher_profiles.json");
        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut profiles: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        let mut fixed = 0usize;
        if let Some(map) = profiles.get_mut("profiles").and_then(|p| p.as_object_mut()) {
            for (_key, prof) in map.iter_mut() {
                let Some(last_id) = prof.get("lastVersionId").and_then(|v| v.as_str()) else { continue };
                if !last_id.starts_with("rsift-loader-") { continue }
                let version_dir = minecraft_dir.join("versions").join(last_id);
                let dll = version_dir.join("rsift_jvm.dll");
                if !dll.exists() { continue }
                let install_root = Self::detect_install_root(
                    &version_dir,
                    prof.get("javaArgs").and_then(|v| v.as_str()),
                );
                let new_args = Self::canonical_java_args(&version_dir, last_id, &install_root);
                let game_dir = minecraft_dir.to_string_lossy().to_string();
                let mut changed = false;
                if prof.get("javaArgs").and_then(|v| v.as_str()) != Some(new_args.as_str()) {
                    prof["javaArgs"] = json!(new_args);
                    changed = true;
                }
                if prof.get("gameDir").and_then(|v| v.as_str()) != Some(game_dir.as_str()) {
                    prof["gameDir"] = json!(game_dir);
                    changed = true;
                }
                if changed {
                    fixed += 1;
                }
            }
        }
        if fixed > 0 {
            fs::write(&path, serde_json::to_string_pretty(&profiles).unwrap())
                .map_err(|e| e.to_string())?;
        }
        Ok(fixed)
    }

    fn detect_install_root(version_dir: &Path, java_args: Option<&str>) -> PathBuf {
        if let Some(args) = java_args {
            for part in args.split_whitespace() {
                if let Some(p) = part.strip_prefix("-Drsift.native.path=") {
                    return PathBuf::from(p);
                }
            }
        }
        let opts = version_dir.join("rsift-agent.opts");
        if let Ok(text) = fs::read_to_string(&opts) {
            for line in text.lines() {
                if let Some(p) = line.strip_prefix("nativePath=") {
                    return PathBuf::from(p.trim());
                }
            }
        }
        PathBuf::from(".")
    }

    /// Strip -agentpath/-javaagent from rsift version JSON files (profile javaArgs owns launch agents).
    pub fn repair_version_jsons(minecraft_dir: &Path) -> Result<usize, String> {
        let versions_dir = minecraft_dir.join("versions");
        if !versions_dir.is_dir() {
            return Ok(0);
        }
        let mut fixed = 0usize;
        for entry in fs::read_dir(&versions_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let version_id = entry.file_name().to_string_lossy().to_string();
            if !version_id.starts_with("rsift-loader-") {
                continue;
            }
            let json_path = entry.path().join(format!("{}.json", version_id));
            if !json_path.is_file() {
                continue;
            }
            let content = fs::read_to_string(&json_path).map_err(|e| e.to_string())?;
            let mut doc: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
            let Some(jvm) = doc
                .pointer_mut("/arguments/jvm")
                .and_then(|v| v.as_array_mut())
            else {
                continue;
            };
            let before = jvm.len();
            jvm.retain(|v| {
                v.as_str()
                    .map(|s| s.starts_with("-Drsift."))
                    .unwrap_or(false)
            });
            if jvm.len() != before {
                fs::write(&json_path, serde_json::to_string_pretty(&doc).unwrap())
                    .map_err(|e| e.to_string())?;
                fixed += 1;
            }
        }
        Ok(fixed)
    }

    fn write_agent_opts(&self, version_dir: &Path, version_id: &str, install_root: &Path) -> Result<(), String> {
        let mods_dir = self.shared_mods_dir();
        let content = format!(
            "versionId={}\ngameDir={}\nmodDir={}\nnativePath={}\n",
            version_id,
            self.minecraft_dir.display(),
            mods_dir.display(),
            install_root.display()
        );
        let path = version_dir.join("rsift-agent.opts");
        fs::write(&path, content).map_err(|e| format!("write agent opts: {}", e))?;
        info!("[Install] agent opts     = {:?}", path);
        Ok(())
    }

    fn ensure_valid_bootstrap_jar(&self, install_root: &Path) {
        let prebuilt = install_root.join("bootstrap").join("prebuilt").join("rsift-bootstrap.jar");
        if prebuilt.is_file() {
            return;
        }
        let script = install_root.join("bootstrap").join("pack-jar.ps1");
        if !script.exists() {
            return;
        }
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-WindowStyle", "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        match cmd.output() {
            Ok(o) if o.status.success() => {
                info!("[Install] bootstrap jar rebuilt via pack-jar.ps1");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                warn!("[Install] pack-jar.ps1 failed (using prebuilt if present): {}", stderr);
            }
            Err(e) => warn!("[Install] could not run pack-jar.ps1: {}", e),
        }
    }

    fn deploy_bootstrap_jar(&self, install_root: &Path, version_dir: &Path) -> Result<Option<PathBuf>, String> {
        let _ = self.ensure_valid_bootstrap_jar(install_root);
        let dest = version_dir.join("rsift-bootstrap.jar");
        let candidates = [
            install_root.join("target").join("bootstrap").join("rsift-bootstrap.jar"),
            install_root.join("bootstrap").join("prebuilt").join("rsift-bootstrap.jar"),
            install_root.join("bootstrap").join("rsift-bootstrap.jar"),
            install_root.join("rsift-bootstrap.jar"),
        ];
        for src in &candidates {
            if src.exists() {
                fs::copy(src, &dest).map_err(|e| format!("copy bootstrap jar: {}", e))?;
                info!("[Install] bootstrap_jar   = {:?} → {:?}", src, dest);
                return Ok(Some(dest));
            }
        }
        match EmbeddedPayloads::deploy_bootstrap_jar(version_dir) {
            Ok(p) => Ok(Some(p)),
            Err(e) => {
                warn!(
                    "[Install] bootstrap_jar   = 配備不可 — JVM フックは無効化されます: {}",
                    e
                );
                Ok(None)
            }
        }
    }

    fn deploy_official_dll_mods(&self, source_dir: &Path, version_dir: &Path) -> Result<Vec<String>, String> {
        let shared_mods_dir = self.shared_mods_dir();
        let version_mods_dir = version_dir.join("mods");
        fs::create_dir_all(&shared_mods_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&version_mods_dir).map_err(|e| e.to_string())?;

        let mut deployed = Vec::new();
        for dll in Self::official_mod_dlls() {
            let candidates = [
                source_dir.join("mods").join(dll),
                source_dir.join("target/release").join(dll),
                source_dir.join(dll),
            ];
            let mut found = false;
            for path in &candidates {
                if path.exists() {
                    let shared_dest = shared_mods_dir.join(dll);
                    let version_dest = version_mods_dir.join(dll);
                    fs::copy(path, &shared_dest).map_err(|e| format!("copy mod {}: {}", dll, e))?;
                    fs::copy(path, &version_dest).map_err(|e| format!("copy mod {}: {}", dll, e))?;
                    info!("[Install] mod deployed from disk = {:?} → {:?}", path, shared_dest);
                    deployed.push(dll.to_string());
                    found = true;
                    break;
                }
            }
            if !found {
                // 自己完結 setup: 実埋め込み payload から配備する。
                match EmbeddedPayloads::deploy_single_mod(&shared_mods_dir, dll) {
                    Ok(path) => {
                        let version_dest = version_mods_dir.join(dll);
                        fs::copy(&path, &version_dest)
                            .map_err(|e| format!("copy mod {} to version mods: {}", dll, e))?;
                        info!(
                            "[Install] mod deployed from embedded setup payload -> {:?}",
                            path
                        );
                        deployed.push(dll.to_string());
                    }
                    Err(e) => warn!("[Install] mod {} を配備できません: {}", dll, e),
                }
            }
        }

        if deployed.is_empty() {
            // Windows では公式 mod DLL がインストールの核心価値 — 0 件配備は fail-loud Err
            // (旧実装は偽マーカー文字列を書き込んで「成功」偽装していた)。
            // 非 Windows (開発/CI) では embedded payload 非対応のため warn のみで継続。
            if cfg!(target_os = "windows") {
                return Err(
                    "公式 mod DLL を 1 件も配備できませんでした (disk にも embedded にも非存在)。\
                     先に `cargo build --release -p rsgraphics -p rscalc -p rsreplay` を実行してから \
                     installer を再ビルド/再実行すること"
                        .to_string(),
                );
            }
            warn!("[Install] 公式 mod の embedded payload は Windows DLL のみ対応 — 今回は 0 件配備で継続");
        }

        Ok(deployed)
    }

    fn shared_mods_dir(&self) -> PathBuf {
        self.minecraft_dir.join("mods")
    }

    /// Copy D3D12 Agility SDK redist next to game binaries (`D3D12/` + `D3D12Core.dll`).
    fn deploy_agility_sdk(&self, install_root: &Path, version_dir: &Path) -> Result<(), String> {
        if !cfg!(target_os = "windows") {
            return Ok(());
        }
        let dest = version_dir.join("D3D12");
        let candidates = [
            install_root.join("third_party").join("D3D12"),
            install_root.join("D3D12"),
            std::env::var("RSIFT_D3D12_SDK_SOURCE")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_default(),
        ];
        for src in &candidates {
            if src.as_os_str().is_empty() || !src.is_dir() {
                continue;
            }
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if path.is_file() {
                    let name = path.file_name().unwrap();
                    fs::copy(&path, dest.join(name)).map_err(|e| e.to_string())?;
                }
            }
            info!("[Install] agility_sdk     = {:?} → {:?}", src, dest);
            return Ok(());
        }
        warn!(
            "[Install] D3D12 Agility folder not found — place SDK at third_party/D3D12 or set RSIFT_D3D12_SDK_SOURCE"
        );
        Ok(())
    }

    pub fn official_mod_dlls() -> &'static [&'static str] {
        if cfg!(target_os = "windows") {
            &["rscalc.dll", "rsgraphics.dll", "rsreplay.dll"]
        } else if cfg!(target_os = "macos") {
            &["librscalc.dylib", "librsgraphics.dylib", "librsreplay.dylib"]
        } else {
            &["librscalc.so", "librsgraphics.so", "librsreplay.so"]
        }
    }

    fn register_profile(
        &self,
        profiles_path: &Path,
        version_id: &str,
        game_dir: &Path,
        java_args: &str,
    ) -> Result<String, String> {
        let content = fs::read_to_string(profiles_path).unwrap_or_else(|_| "{\"profiles\":{}}".to_string());
        let mut profiles: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({"profiles":{}}));

        let base_name = format!("Rsift Loader {} ({})", self.target_version, self.build_version);
        let mut final_name = base_name.clone();

        if let Some(map) = profiles.get_mut("profiles").and_then(|p| p.as_object_mut()) {
            let existing: Vec<String> = map
                .values()
                .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect();
            final_name = Self::unique_profile_name(&existing, &base_name);
            let profile_key = version_id.to_string();
            let profile = json!({
                "name": final_name,
                "type": "custom",
                "created": "2026-07-05T00:00:00.000Z",
                "lastUsed": "2026-07-05T00:00:00.000Z",
                "icon": "Grass",
                "lastVersionId": version_id,
                "gameDir": game_dir.to_string_lossy(),
                "javaArgs": java_args
            });
            map.insert(profile_key, profile);

            info!("[Install] profile name    = {}", final_name);
            info!("[Install] lastVersionId   = {}", version_id);
            info!("[Install] gameDir         = {:?}", game_dir);
            info!("[Install] javaArgs        = {}", java_args);
        }

        fs::write(profiles_path, serde_json::to_string_pretty(&profiles).unwrap())
            .map_err(|e| format!("write profiles: {}", e))?;
        Ok(final_name)
    }

    fn write_install_manifest(&self, version_dir: &Path, result: &InstallResult) -> Result<(), String> {
        let enabled: Vec<_> = result
            .engine_caps
            .features
            .enabled
            .iter()
            .map(|f| format!("{:?}", f))
            .collect();
        let manifest = json!({
            "version_id": result.version_id,
            "inherits_from": result.inherits_from,
            "build_version": self.build_version,
            "version_json": result.version_json.to_string_lossy(),
            "version_jar": result.version_jar.to_string_lossy(),
            "agent_path": result.agent_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "agent_loaded": result.agent_loaded,
            "mods_deployed": result.mods_deployed,
            "profile_name": result.profile_name,
            "verify_log_line": "[Rsift] Agent_OnLoad fallback (-agentpath mode)",
            "render_engine": {
                "backend": result.engine_caps.render_backend.as_str(),
                "shader_model": result.engine_caps.shader_model.as_str(),
                "agility_sdk": result.engine_caps.agility_sdk,
                "gpu_upload_heaps": result.engine_caps.gpu_upload_heaps,
                "enhanced_barriers": result.engine_caps.enhanced_barriers,
                "gpu_name": result.engine_caps.probe.gpu_name,
                "gpu_score": result.engine_caps.probe.gpu_score,
                "vram_mb": result.engine_caps.probe.vram_mb,
                "sm69_eligible": result.engine_caps.probe.sm69_eligible,
                "features_enabled": enabled,
                "feature_count": result.engine_caps.features.count(),
            }
        });
        let path = version_dir.join("rsift-install-manifest.json");
        fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).map_err(|e| e.to_string())?;
        info!("[Install] manifest        = {:?}", path);
        Ok(())
    }

    fn register_to_system_path(&self, install_dir: &Path) -> Result<(), String> {
        let abs_path = install_dir.canonicalize().unwrap_or_else(|_| install_dir.to_path_buf());
        let path_str = abs_path.to_string_lossy().to_string();
        if cfg!(target_os = "windows") {
            let ps_cmd = format!(
                "$old = [Environment]::GetEnvironmentVariable('Path', 'User'); \
                 if ($old -notlike '*{}*') {{ \
                     [Environment]::SetEnvironmentVariable('Path', $old + ';{}', 'User'); \
                 }}",
                path_str, path_str
            );
            use std::os::windows::process::CommandExt;
            let mut cmd = Command::new("powershell");
            cmd.args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &ps_cmd,
            ]);
            cmd.creation_flags(0x08000000);
            let _ = cmd.output();
        }
        Ok(())
    }
}
