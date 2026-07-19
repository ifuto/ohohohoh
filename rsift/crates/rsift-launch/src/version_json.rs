//! Merge `inheritsFrom` version JSON chains (vanilla + rsift loader).

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MergedVersion {
    pub id: String,
    pub main_class: String,
    pub minecraft_dir: PathBuf,
    pub game_dir: PathBuf,
    pub jar: PathBuf,
    pub libraries: Vec<LibraryEntry>,
    pub jvm_args: Vec<String>,
    pub game_arg_template: Vec<String>,
    pub asset_index: String,
    pub java_major: u32,
}

#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub name: String,
    pub path: PathBuf,
    pub natives: Option<HashMap<String, String>>,
}

pub fn load_merged_version(minecraft_dir: &Path, version_id: &str) -> Result<MergedVersion, String> {
    let versions_dir = minecraft_dir.join("versions");
    let version_dir = versions_dir.join(version_id);
    let version_json_path = version_dir.join(format!("{version_id}.json"));
    if !version_json_path.is_file() {
        return Err(format!("version json missing: {:?}", version_json_path));
    }
    let root: Value = serde_json::from_str(
        &fs::read_to_string(&version_json_path).map_err(|e| format!("read version json: {e}"))?,
    )
    .map_err(|e| format!("parse version json: {e}"))?;

    let mut chain = Vec::new();
    collect_inheritance(&versions_dir, &root, version_id, &mut chain)?;

    let mut libraries = Vec::new();
    let mut jvm_args = Vec::new();
    let mut game_args = Vec::new();
    let mut main_class = "net.minecraft.client.main.Main".to_string();
    let mut asset_index = version_id.to_string();
    let mut java_major = 21u32;

    for (id, doc) in &chain {
        if let Some(mc) = doc.get("mainClass").and_then(|v| v.as_str()) {
            main_class = mc.to_string();
        }
        if let Some(jv) = doc.get("javaVersion").and_then(|v| v.get("majorVersion")) {
            if let Some(m) = jv.as_u64() {
                java_major = m as u32;
            }
        }
        if let Some(ai) = doc.get("assetIndex").and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
            asset_index = ai.to_string();
        } else if let Some(ai) = doc.get("assets").and_then(|v| v.as_str()) {
            asset_index = ai.to_string();
        }
        libraries.extend(parse_libraries(minecraft_dir, doc)?);
        jvm_args.extend(parse_arguments(doc.get("arguments").and_then(|a| a.get("jvm"))));
        game_args.extend(parse_arguments(doc.get("arguments").and_then(|a| a.get("game"))));
        if let Some(args) = doc.get("minecraftArguments").and_then(|v| v.as_str()) {
            for part in args.split_whitespace() {
                game_args.push(part.to_string());
            }
        }
        let _ = id;
    }

    dedup_libraries(&mut libraries);

    let jar = version_dir.join(format!("{version_id}.jar"));
    if !jar.is_file() {
        return Err(format!("version jar missing: {:?}", jar));
    }

    Ok(MergedVersion {
        id: version_id.to_string(),
        main_class,
        minecraft_dir: minecraft_dir.to_path_buf(),
        game_dir: version_dir,
        jar,
        libraries,
        jvm_args,
        game_arg_template: game_args,
        asset_index,
        java_major,
    })
}

fn collect_inheritance(
    versions_dir: &Path,
    doc: &Value,
    id: &str,
    out: &mut Vec<(String, Value)>,
) -> Result<(), String> {
    if let Some(parent) = doc.get("inheritsFrom").and_then(|v| v.as_str()) {
        let parent_json = versions_dir.join(parent).join(format!("{parent}.json"));
        let parent_doc: Value = serde_json::from_str(
            &fs::read_to_string(&parent_json)
                .map_err(|e| format!("read parent {parent}: {e}"))?,
        )
        .map_err(|e| format!("parse parent {parent}: {e}"))?;
        collect_inheritance(versions_dir, &parent_doc, parent, out)?;
    }
    out.push((id.to_string(), doc.clone()));
    Ok(())
}

fn parse_libraries(minecraft_dir: &Path, doc: &Value) -> Result<Vec<LibraryEntry>, String> {
    let Some(libs) = doc.get("libraries").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for lib in libs {
        if let Some(e) = library_from_modern(minecraft_dir, lib)? {
            out.push(e);
        }
    }
    Ok(out)
}

fn library_from_modern(minecraft_dir: &Path, lib: &Value) -> Result<Option<LibraryEntry>, String> {
    if !rule_allows(lib) {
        return Ok(None);
    }
    let name = lib
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("library without name")?;
    let path = maven_to_path(name);
    let full = minecraft_dir.join("libraries").join(&path);
    if !full.is_file() {
        return Ok(None);
    }
    let natives = lib.get("natives").and_then(|n| {
        let mut m = HashMap::new();
        if let Some(obj) = n.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    let key = if cfg!(windows) {
                        k.replace("${arch}", "x86_64")
                    } else if cfg!(target_os = "macos") {
                        k.replace("${arch}", "x86_64")
                    } else {
                        k.replace("${arch}", "x86_64")
                    };
                    m.insert(k.clone(), s.replace("${arch}", "x86_64"));
                }
            }
        }
        if m.is_empty() { None } else { Some(m) }
    });
    Ok(Some(LibraryEntry {
        name: name.to_string(),
        path: full,
        natives,
    }))
}

fn library_from_legacy(minecraft_dir: &Path, lib: &Value) -> Result<Option<LibraryEntry>, String> {
    library_from_modern(minecraft_dir, lib)
}

fn rule_allows(lib: &Value) -> bool {
    let Some(rules) = lib.get("rules").and_then(|v| v.as_array()) else {
        return true;
    };
    let mut allow = false;
    let mut matched = false;
    for rule in rules {
        let action_allow = rule.get("action").and_then(|v| v.as_str()) != Some("disallow");
        let os_name = rule
            .get("os")
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str());
        let os_ok = match os_name {
            Some("windows") => cfg!(windows),
            Some("osx") => cfg!(target_os = "macos"),
            Some("linux") => cfg!(target_os = "linux"),
            _ => true,
        };
        let features_ok = rule
            .get("features")
            .map(features_match_offline)
            .unwrap_or(true);
        if os_ok && features_ok {
            matched = true;
            allow = action_allow;
        }
    }
    matched && allow
}

/// Mojang `arguments.game` feature flags for offline/custom launcher (no quick play, custom res on).
fn offline_feature_enabled(name: &str) -> bool {
    match name {
        "is_demo_user" => false,
        "has_custom_resolution" => true,
        "has_quick_plays_support" => false,
        "is_quick_play_singleplayer" => false,
        "is_quick_play_multiplayer" => false,
        "is_quick_play_realms" => false,
        _ => false,
    }
}

fn features_match_offline(features: &Value) -> bool {
    let Some(obj) = features.as_object() else {
        return true;
    };
    for (key, want) in obj {
        let required = want.as_bool().unwrap_or(true);
        if offline_feature_enabled(key) != required {
            return false;
        }
    }
    true
}

fn parse_arguments(node: Option<&Value>) -> Vec<String> {
    let Some(node) = node else { return Vec::new() };
    let Some(arr) = node.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            out.push(s.to_string());
            continue;
        }
        let Some(obj) = item.as_object() else { continue };
        if !rule_allows(&Value::Object(obj.clone())) {
            continue;
        }
        match obj.get("value") {
            Some(Value::String(s)) => out.push(s.clone()),
            Some(Value::Array(vals)) => {
                for v in vals {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn maven_to_path(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return name.replace('.', "/");
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3);
    match classifier {
        Some(c) => format!("{group}/{artifact}/{version}/{artifact}-{version}-{c}.jar"),
        None => format!("{group}/{artifact}/{version}/{artifact}-{version}.jar"),
    }
}

fn dedup_libraries(libs: &mut Vec<LibraryEntry>) {
    let mut seen = std::collections::HashSet::new();
    libs.retain(|l| seen.insert(l.path.clone()));
}
