//! Offline launch via `JNI_CreateJavaVM` + `Main.main`.

use std::path::PathBuf;
use std::process::Command;
use tracing::info;

use rsift_api::engine_caps::{EngineCaps, GpuCapabilityProbe};
use rsift_jvm::{JvmConfig, ManagedJvm};

use crate::classpath::{build_classpath, resolve_natives_dir};
use crate::java::ensure_java_home;
use crate::version_json::{load_merged_version, MergedVersion};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LaunchProfile {
    pub id: String,
    pub name: String,
    pub version_id: String,
    pub username: String,
    pub uuid: String,
    pub min_heap: String,
    pub max_heap: String,
    pub width: u32,
    pub height: u32,
    pub extra_jvm_args: Vec<String>,
}

impl Default for LaunchProfile {
    fn default() -> Self {
        Self {
            id: uuid_simple(),
            name: "Rsift オフライン".into(),
            version_id: String::new(),
            username: "RsiftPlayer".into(),
            uuid: offline_uuid("RsiftPlayer"),
            min_heap: "-Xms4G".into(),
            max_heap: "-Xmx8G".into(),
            width: 1920,
            height: 1080,
            extra_jvm_args: vec![
                "-XX:+UseZGC".into(),
                "-XX:+ZGenerational".into(),
                "-XX:+UnlockExperimentalVMOptions".into(),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OfflineLaunchConfig {
    pub minecraft_dir: PathBuf,
    pub install_root: PathBuf,
    pub profile: LaunchProfile,
}

pub fn launch_offline(cfg: OfflineLaunchConfig) -> Result<(), String> {
    let version = load_merged_version(&cfg.minecraft_dir, &cfg.profile.version_id)?;
    ensure_java_home(&cfg.minecraft_dir, version.java_major)?;
    let classpath = build_classpath(&version);
    let natives = resolve_natives_dir(&version)?;
    let game_args = build_game_args(&version, &cfg.profile);

    let agent_dll = cfg
        .install_root
        .join("target/release/rsift_jvm.dll");
    let agent_dll = if agent_dll.is_file() {
        agent_dll
    } else {
        version.game_dir.join("rsift_jvm.dll")
    };
    if !agent_dll.is_file() {
        return Err(format!(
            "rsift_jvm.dll not found — run インストール tab first ({:?})",
            agent_dll
        ));
    }
    let _ = agent_dll;

    let probe = GpuCapabilityProbe::probe();
    let caps = EngineCaps::install_default(&probe, None)?;

    let mut extra = cfg.profile.extra_jvm_args.clone();
    extra.push(format!("-Djava.library.path={}", natives.display()));
    extra.push("-Drsift.loader.enabled=true".into());
    extra.push("-Drsift.native.injection=true".into());
    extra.push(format!("-Drsift.render.backend=dx12_agility"));
    extra.push(format!("-Drsift.shader_model={}", caps.shader_model.as_str()));
    extra.push("-Drsift.agility_sdk=true".into());
    extra.push(format!(
        "-Drsift.version.id={}",
        cfg.profile.version_id
    ));
    extra.push(format!(
        "-Drsift.native.path={}",
        cfg.install_root.display()
    ));
    // agentpath registered by ManagedJvm::launch (rsift_jvm.dll)

  let jvm_config = JvmConfig {
        classpath,
        min_heap: cfg.profile.min_heap.clone(),
        max_heap: cfg.profile.max_heap.clone(),
        max_direct_memory: "-XX:MaxDirectMemorySize=4G".into(),
        extra_jvm_args: extra,
        agent_path: Some(format!(
            "{}=gameDir={},modDir={}/mods",
            agent_dll.display(),
            version.game_dir.display(),
            version.game_dir.display()
        )),
    };

    info!(
        "[RsiftLaunch] offline profile={} version={} user={}",
        cfg.profile.name, cfg.profile.version_id, cfg.profile.username
    );
    info!("[RsiftLaunch] game args ({}): {:?}", game_args.len(), game_args);

    let jvm = ManagedJvm::launch(jvm_config).map_err(|e| e.to_string())?;
    let args: Vec<&str> = game_args.iter().map(|s| s.as_str()).collect();
    jvm.start_minecraft_client(&args)
}

pub(crate) fn build_game_args(version: &MergedVersion, profile: &LaunchProfile) -> Vec<String> {
    let assets_dir = version.minecraft_dir.join("assets");
    let game_dir = version.game_dir.display().to_string();
    let assets = assets_dir.display().to_string();

    if version.game_arg_template.is_empty() {
        return vec![
            "--username".into(),
            profile.username.clone(),
            "--version".into(),
            version.id.clone(),
            "--gameDir".into(),
            game_dir,
            "--assetsDir".into(),
            assets,
            "--assetIndex".into(),
            version.asset_index.clone(),
            "--uuid".into(),
            profile.uuid.clone(),
            "--accessToken".into(),
            "0".into(),
            "--userType".into(),
            "legacy".into(),
            "--width".into(),
            profile.width.to_string(),
            "--height".into(),
            profile.height.to_string(),
        ];
    }

    let mut out = Vec::new();
    for token in &version.game_arg_template {
        let v = substitute_value(token, profile, version, &game_dir, &assets);
        if v.starts_with("${") || v.is_empty() {
            continue;
        }
        out.push(v);
    }
    out
}

fn substitute_value(
    token: &str,
    profile: &LaunchProfile,
    version: &MergedVersion,
    game_dir: &str,
    assets: &str,
) -> String {
    match token {
        "${auth_player_name}" => profile.username.clone(),
        "${auth_uuid}" | "${uuid}" => profile.uuid.clone(),
        "${auth_access_token}" | "${accessToken}" => "0".into(),
        "${auth_session}" => "0".into(),
        "${auth_xuid}" => "0".into(),
        "${clientid}" => "0".into(),
        "${user_type}" => "legacy".into(),
        "${user_properties}" | "${profile_properties}" => "{}".into(),
        "${version_name}" => version.id.clone(),
        "${version_type}" => "release".into(),
        "${game_directory}" | "${gameDir}" => game_dir.to_string(),
        "${assets_root}" | "${assetsDir}" => assets.to_string(),
        "${assets_index_name}" | "${assetIndex}" => version.asset_index.clone(),
        "${resolution_width}" => profile.width.to_string(),
        "${resolution_height}" => profile.height.to_string(),
        "${quickPlayPath}" | "${quickPlaySingleplayer}" | "${quickPlayMultiplayer}"
        | "${quickPlayRealms}" => String::new(),
        _ => token.to_string(),
    }
}

fn uuid_simple() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

/// Offline UUID v3-style from username (stable).
pub fn offline_uuid(username: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    username.hash(&mut h);
    let n = h.finish();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (n >> 32) as u32,
        ((n >> 16) & 0xffff) as u16,
        (n & 0xffff) as u16 | 0x4000,
        ((n >> 48) & 0x3fff) as u16 | 0x8000,
        n & 0xffffffffffff
    )
}

/// Find `java` executable (JAVA_HOME or PATH).
pub fn find_java() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let exe = PathBuf::from(home).join("bin").join(if cfg!(windows) {
            "java.exe"
        } else {
            "java"
        });
        if exe.is_file() {
            return Some(exe);
        }
    }
    which_java()
}

fn which_java() -> Option<PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let out = Command::new(cmd).arg("java").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let first = line.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(PathBuf::from(first))
    }
}

pub fn list_rsift_versions(minecraft_dir: &std::path::Path) -> Vec<String> {
    let versions = minecraft_dir.join("versions");
    let Ok(read) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    read.filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("rsift-loader-")
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn game_args_exclude_quick_play() {
        let mc = PathBuf::from(std::env::var("APPDATA").unwrap()).join(".minecraft");
        let versions = list_rsift_versions(&mc);
        let Some(id) = versions.last().cloned() else {
            return;
        };
        let version = load_merged_version(&mc, &id).expect("version");
        let args = build_game_args(&version, &LaunchProfile::default());
        assert!(
            !args.iter().any(|a| a.contains("quickPlay")),
            "quick play args must be filtered: {args:?}"
        );
        assert!(args.windows(2).any(|w| w[0] == "--username"), "{args:?}");
    }
}
