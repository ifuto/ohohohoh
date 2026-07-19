//! Vanilla launcher–style subprocess boot.
//!
//! Assembles the exact same command line the official Minecraft launcher
//! would run, then spawns `javaw` / `java` as a **standalone child process**.
//!
//! - No `JNI_CreateJavaVM` (in-process JVM is a separate, later path)
//! - No `rsift_jvm.dll` required — a plain `.minecraft` with 1.21.11 is enough
//! - The launcher stays light; the game keeps running even if the launcher closes

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::classpath::{build_classpath, resolve_natives_dir};
use crate::java::detect_java_home;
use crate::offline::{build_game_args, OfflineLaunchConfig};
use crate::version_json::{load_merged_version, MergedVersion};

/// A fully-prepared launch command. Cheap to hold; call [`LaunchPlan::spawn`]
/// on the UI thread so the launcher owns the child process.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub java: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub cmdline: String,
}

impl LaunchPlan {
    pub fn spawn(&self) -> Result<Child, String> {
        Command::new(&self.java)
            .args(&self.args)
            .current_dir(&self.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("java の起動に失敗しました ({:?}): {}", self.java, e))
    }
}

/// Resolve version JSON chain, natives and Java, then build the full command
/// line. Designed to run on a background thread (may touch disk for ~1–2s).
pub fn prepare_vanilla_launch(
    cfg: &OfflineLaunchConfig,
    java_override: &str,
) -> Result<LaunchPlan, String> {
    let version = load_merged_version(&cfg.minecraft_dir, &cfg.profile.version_id)?;
    let java = resolve_java(cfg, &version, java_override)?;
    let classpath = build_classpath(&version);
    let natives = resolve_natives_dir(&version)?;

    let mut args: Vec<String> = Vec::new();
    // Profile-controlled heap (version-json -Xmx/-Xms tokens are filtered below)
    args.push(cfg.profile.min_heap.clone());
    args.push(cfg.profile.max_heap.clone());
    for a in &cfg.profile.extra_jvm_args {
        args.push(a.clone());
    }

    let mut has_cp_flag = false;
    for token in &version.jvm_args {
        let v = substitute_jvm(token, &classpath, &natives);
        if v.is_empty() || v.contains("${") {
            continue;
        }
        if v.starts_with("-Xms") || v.starts_with("-Xmx") {
            continue; // profile owns heap
        }
        if v == "-cp" || v == "-classpath" {
            has_cp_flag = true;
        }
        args.push(v);
    }
    if !has_cp_flag {
        args.push("-cp".into());
        args.push(classpath.clone());
    }

    args.push(version.main_class.clone());
    args.extend(build_game_args(&version, &cfg.profile));

    let cmdline = format!("{} {}", java.display(), args.join(" "));
    Ok(LaunchPlan {
        java,
        args,
        cwd: version.minecraft_dir.clone(),
        cmdline,
    })
}

fn substitute_jvm(token: &str, classpath: &str, natives: &Path) -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let natives_s = natives.to_string_lossy().into_owned();
    token
        .replace("${natives_directory}", &natives_s)
        .replace("${classpath}", classpath)
        .replace("${classpath_separator}", sep)
        .replace("${launcher_name}", "Rsift Launcher")
        .replace("${launcher_version}", "1.0.0")
        .replace("${rundir}", ".")
}

fn resolve_java(
    cfg: &OfflineLaunchConfig,
    version: &MergedVersion,
    java_override: &str,
) -> Result<PathBuf, String> {
    let candidate = java_override.trim();
    if !candidate.is_empty() {
        let p = PathBuf::from(candidate);
        let exe = if p.is_dir() {
            p.join("bin").join(java_bin_name())
        } else {
            p
        };
        if exe.is_file() {
            return Ok(prefer_javaw(&exe));
        }
        return Err(format!(
            "指定された Java が見つかりません: {}",
            exe.display()
        ));
    }

    let home = detect_java_home(&cfg.minecraft_dir, version.java_major).ok_or_else(|| {
        format!(
            "Java {}+ が検出できません。公式ランチャーで一度起動してランタイムを入れるか、設定からパスを指定してください。",
            version.java_major
        )
    })?;
    Ok(prefer_javaw(&home.join("bin").join(java_bin_name())))
}

fn java_bin_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// `javaw.exe` avoids a stray console window on Windows, just like the
/// official launcher. Falls back to `java(.exe)` when not present.
fn prefer_javaw(java: &Path) -> PathBuf {
    if cfg!(windows) {
        let javaw = java.with_file_name("javaw.exe");
        if javaw.is_file() {
            return javaw;
        }
    }
    java.to_path_buf()
}

/// All versions this launcher can boot: `rsift-loader-*` first, then plain
/// vanilla directories. A version needs its `<id>.json` (and, unless it is an
/// rsift loader chain that inherits the jar, its `<id>.jar`).
pub fn list_launchable_versions(minecraft_dir: &Path) -> Vec<String> {
    let versions = minecraft_dir.join("versions");
    let Ok(read) = fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut rsift = Vec::new();
    let mut plain = Vec::new();
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        if !dir.join(format!("{name}.json")).is_file() {
            continue;
        }
        let is_rsift = name.starts_with("rsift-loader-");
        if !is_rsift && !dir.join(format!("{name}.jar")).is_file() {
            continue;
        }
        if is_rsift {
            rsift.push(name);
        } else {
            plain.push(name);
        }
    }
    rsift.sort();
    plain.sort();
    rsift.extend(plain);
    rsift
}
