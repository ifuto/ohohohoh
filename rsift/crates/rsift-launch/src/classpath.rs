//! Classpath + natives extraction.

use crate::version_json::{LibraryEntry, MergedVersion};
use std::path::{Path, PathBuf};

pub fn build_classpath(version: &MergedVersion) -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut paths = vec![version.jar.clone()];
    for lib in &version.libraries {
        paths.push(lib.path.clone());
    }
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

pub fn resolve_natives_dir(version: &MergedVersion) -> Result<PathBuf, String> {
    let natives_root = version
        .game_dir
        .join("natives")
        .join(&version.id);
    std::fs::create_dir_all(&natives_root).map_err(|e| format!("mkdir natives: {e}"))?;

    for lib in &version.libraries {
        if let Some(natives) = &lib.natives {
            extract_natives_from_jar(&lib.path, natives, &natives_root)?;
        }
    }
    Ok(natives_root)
}

fn extract_natives_from_jar(
    jar: &Path,
    natives: &std::collections::HashMap<String, String>,
    out_dir: &Path,
) -> Result<(), String> {
    let os_key = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    };
    let Some(classifier) = natives.get(os_key) else {
        return Ok(());
    };
    let file = std::fs::File::open(jar).map_err(|e| format!("open jar {:?}: {e}", jar))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("zip {:?}: {e}", jar))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if !name.contains(classifier) && !name.ends_with(".dll") && !name.ends_with(".so") {
            continue;
        }
        if name.ends_with('/') {
            continue;
        }
        let file_name = name.rsplit('/').next().unwrap_or(&name);
        let dest = out_dir.join(file_name);
        if dest.is_file() {
            continue;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| format!("create {:?}: {e}", dest))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract {:?}: {e}", dest))?;
    }
    Ok(())
}
