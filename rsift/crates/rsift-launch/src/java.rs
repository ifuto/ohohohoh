//! Resolve a 64-bit JDK/JRE for Minecraft (prefers bundled runtime, then JAVA_HOME).

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Pick `JAVA_HOME` for `JNI_CreateJavaVM`. Sets the process env when found.
pub fn ensure_java_home(minecraft_dir: &Path, required_major: u32) -> Result<PathBuf, String> {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let p = PathBuf::from(&home);
        if java_home_ok(&p, required_major) {
            info!("[RsiftLaunch] JAVA_HOME={}", p.display());
            return Ok(p);
        }
    }

    let mut candidates = Vec::new();
    candidates.extend(minecraft_runtime_javas(minecraft_dir));
    candidates.extend(installed_jdk_roots());

    let mut best: Option<(u32, PathBuf)> = None;
    for root in candidates {
        let Some(major) = java_major_at(&root) else {
            continue;
        };
        if major < required_major {
            continue;
        }
        let replace = match &best {
            None => true,
            Some((bm, _)) => {
                if *bm == required_major {
                    false
                } else if major == required_major {
                    true
                } else {
                    major < *bm
                }
            }
        };
        if replace {
            best = Some((major, root));
        }
    }

    let Some((major, home)) = best else {
        return Err(format!(
            "Java {required_major}+ が見つかりません。JDK {required_major} をインストールするか、\
             公式ランチャーで 1.21.11 を一度起動してランタイムを取得してください。"
        ));
    };

    std::env::set_var("JAVA_HOME", home.as_os_str());
    info!(
        "[RsiftLaunch] using Java {} at {}",
        major,
        home.display()
    );
    Ok(home)
}

fn java_home_ok(home: &Path, required_major: u32) -> bool {
    java_exe(home).is_file()
        && java_major_at(home)
            .map(|m| m >= required_major)
            .unwrap_or(false)
}

fn java_exe(home: &Path) -> PathBuf {
    home.join("bin").join(if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    })
}

fn java_major_at(home: &Path) -> Option<u32> {
    let exe = java_exe(home);
    let out = Command::new(&exe).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    parse_java_major(&text)
}

fn parse_java_major(text: &str) -> Option<u32> {
    let line = text.lines().next()?;
    let ver = line.split('"').nth(1)?;
    if let Some(rest) = ver.strip_prefix("1.") {
        return rest.split('.').next()?.parse().ok();
    }
    ver.split('.').next()?.parse().ok()
}

fn minecraft_runtime_javas(minecraft_dir: &Path) -> Vec<PathBuf> {
    let runtime = minecraft_dir.join("runtime");
    let Ok(read) = std::fs::read_dir(&runtime) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let base = entry.path();
        for sub in [
            "windows-x64",
            "windows",
            "linux",
            "mac-os",
            "mac-os-arm64",
        ] {
            let platform = base.join(sub);
            if !platform.is_dir() {
                continue;
            }
            collect_jre_dirs(&platform, &mut out);
        }
    }
    out
}

fn collect_jre_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.join("bin").join("java.exe").is_file() || dir.join("bin").join("java").is_file() {
        out.push(dir.to_path_buf());
    }
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            if e.path().is_dir() {
                collect_jre_dirs(&e.path(), out);
            }
        }
    }
}

fn installed_jdk_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for base in [
        PathBuf::from(r"C:\Program Files\Java"),
        PathBuf::from(r"C:\Program Files\Eclipse Adoptium"),
        PathBuf::from(r"C:\Program Files\Microsoft"),
        PathBuf::from(r"C:\Program Files\Amazon Corretto"),
        PathBuf::from(r"C:\Program Files\Zulu"),
    ] {
        if let Ok(read) = std::fs::read_dir(&base) {
            for e in read.flatten() {
                let p = e.path();
                if p.is_dir() {
                    roots.push(p);
                }
            }
        }
    }
    roots.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    roots
}

pub fn detect_java_home(minecraft_dir: &Path, required_major: u32) -> Option<PathBuf> {
    ensure_java_home(minecraft_dir, required_major).ok()
}
