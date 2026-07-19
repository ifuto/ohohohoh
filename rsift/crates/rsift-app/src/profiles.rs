//! Profile persistence (local JSON — no GitHub).

use rsift_launch::LaunchProfile;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ProfileStore {
    pub minecraft_dir: PathBuf,
    pub install_root: PathBuf,
    pub profiles: Vec<LaunchProfile>,
    pub selected: usize,
    /// Manual Java path (java.exe or JAVA_HOME dir). Empty = auto-detect.
    #[serde(default)]
    pub java_override: String,
}

impl ProfileStore {
    pub fn path() -> PathBuf {
        dirs_path().join("profiles.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        if path.is_file() {
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(mut s) = serde_json::from_str::<ProfileStore>(&data) {
                    if s.minecraft_dir.as_os_str().is_empty() {
                        s.minecraft_dir = default_minecraft_dir();
                    }
                    if s.install_root.as_os_str().is_empty() {
                        s.install_root = default_install_root();
                    }
                    if s.profiles.is_empty() {
                        s.profiles.push(default_profile(&s));
                    }
                    return s;
                }
            }
        }
        let mut s = ProfileStore {
            minecraft_dir: default_minecraft_dir(),
            install_root: default_install_root(),
            profiles: vec![],
            selected: 0,
        };
        s.profiles.push(default_profile(&s));
        s
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn selected_profile(&self) -> &LaunchProfile {
        &self.profiles[self.selected.min(self.profiles.len().saturating_sub(1))]
    }

    pub fn selected_profile_mut(&mut self) -> &mut LaunchProfile {
        let idx = self.selected.min(self.profiles.len().saturating_sub(1));
        &mut self.profiles[idx]
    }

    pub fn refresh_versions(&mut self) {
        let versions = rsift_launch::list_launchable_versions(&self.minecraft_dir);
        if let Some(latest) = versions.last() {
            let p = self.selected_profile_mut();
            if p.version_id.is_empty() {
                p.version_id = latest.clone();
            }
        }
    }
}

fn default_minecraft_dir() -> PathBuf {
    rsift_installer::LauncherInstaller::detect_minecraft_dir()
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_install_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_profile(store: &ProfileStore) -> LaunchProfile {
    let mut p = LaunchProfile::default();
    let versions = rsift_launch::list_launchable_versions(&store.minecraft_dir);
    if let Some(v) = versions.last() {
        p.version_id = v.clone();
        p.name = format!("Rsift {v}");
    }
    p
}

fn dirs_path() -> PathBuf {
    if cfg!(windows) {
        std::env::var("APPDATA")
            .map(|a| PathBuf::from(a).join(".rsift-launcher"))
            .unwrap_or_else(|_| PathBuf::from(".rsift-launcher"))
    } else {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".rsift-launcher"))
            .unwrap_or_else(|_| PathBuf::from(".rsift-launcher"))
    }
}

pub fn detect_install_root() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..5 {
            if let Some(ref d) = dir {
                if d.join("Cargo.toml").is_file() || d.join("target/release/rsift_jvm.dll").is_file()
                {
                    return d.clone();
                }
                dir = d.parent().map(|p| p.to_path_buf());
            }
        }
    }
    default_install_root()
}
