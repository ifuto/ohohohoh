//! # Embedded Binary Payloads (`EmbeddedPayloads`)
//!
//! 単一の `rsift-setup.exe` だけをダウンロードしたユーザーでも
//! 「1クリック全自動導入」が完結するよう、実際にコンパイルされた
//! ネイティブ `.dll` プラグイン (`rsgraphics.dll`, `rscalc.dll`, `rsreplay.dll`)、
//! コア `rsift_jvm.dll`、`rsift-bootstrap.jar` をインストーラーへ **実埋め込み** する。
//!
//! ## 監査対応 (ダミー payload の完全撤廃)
//!
//! 旧実装は 148byte の MZ スタブや `MZ_RSGRAPHICS_...` マーカー文字列を
//! 「自己展開 DLL」として書き出す **偽装** であり、導入先の Minecraft は
//! ロード不能なゴミを拾ってクラッシュしていた。現実装は build.rs が生成する
//! `payloads.rs` 経由で **実バイナリのみ** を埋め込み、ビルド時に実成果物が
//! 存在しなかった payload は `None` → 本モジュールは **fail-loud な `Err`** を返す
//! (呼び出し側は既存の graceful degradation 経路で明示 warn する)。

use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

// build.rs 生成 (実 payload or None)。ダミーは一切含まない。
include!(concat!(env!("OUT_DIR"), "/payloads.rs"));

/// 既存配置ファイルが「実 DLL」と見なせる最小サイズ (実 cdylib は MB 級)。
/// 旧ダミー (148byte MZ スタブ / 数十 byte マーカー文字列) を誤って温存しない。
const MIN_EXISTING_DLL_BYTES: u64 = 100_000;

/// 埋め込みバイナリまたはディスク上の事前コンパイルバイナリを展開・提供する自己展開マネージャー。
pub struct EmbeddedPayloads;

impl EmbeddedPayloads {
    /// 実コンパイル済みの `rsift_jvm.dll` を展開する。
    ///
    /// 既に実ファイルが存在する場合は温存 (上書きしない)。payload 未埋め込み
    /// なら fail-loud Err (呼び出し側が "native hooks disabled" を明示 warn する)。
    pub fn deploy_rsift_jvm(target_dir: &Path) -> Result<PathBuf, String> {
        let dest = target_dir.join("rsift_jvm.dll");
        if dest.exists()
            && fs::metadata(&dest)
                .map(|m| m.len() >= MIN_EXISTING_DLL_BYTES)
                .unwrap_or(false)
        {
            // 旧スタブの温存防止: 先頭 MZ マジックを検証し、偽なら上書きへ進む。
            let real = fs::read(&dest)
                .map(|head| head.starts_with(b"MZ"))
                .unwrap_or(false);
            if real {
                return Ok(dest);
            }
        }
        let payload = PAYLOAD_RSIFT_JVM_DLL.ok_or_else(|| {
            "rsift_jvm.dll payload 未埋め込み — `cargo build --release -p rsift-jvm` 後に installer を再ビルドすること".to_string()
        })?;
        Self::write_verified(&dest, payload, b"MZ")?;
        info!(
            "📦 [Self-Extract] Deployed embedded rsift_jvm.dll ({} bytes) -> {:?}",
            payload.len(),
            dest
        );
        Ok(dest)
    }

    /// 実ビルド済み `rsift-bootstrap.jar` を展開する。未埋め込みなら Err。
    pub fn deploy_bootstrap_jar(target_dir: &Path) -> Result<PathBuf, String> {
        let dest = target_dir.join("rsift-bootstrap.jar");
        if dest.exists() && fs::metadata(&dest).map(|m| m.len() > 100).unwrap_or(false) {
            let real = fs::read(&dest)
                .map(|head| head.starts_with(b"PK\x03\x04"))
                .unwrap_or(false);
            if real {
                return Ok(dest);
            }
        }
        let payload = PAYLOAD_BOOTSTRAP_JAR.ok_or_else(|| {
            "rsift-bootstrap.jar payload 未埋め込み — `bootstrap\\pack-jar.ps1` (要 JDK) 後に installer を再ビルドすること".to_string()
        })?;
        Self::write_verified(&dest, payload, b"PK\x03\x04")?;
        info!(
            "📦 [Self-Extract] Deployed embedded rsift-bootstrap.jar ({} bytes) -> {:?}",
            payload.len(),
            dest
        );
        Ok(dest)
    }

    /// 個別公式 `.dll` モッドを `.minecraft/mods/` へ実ペイロードで展開する。
    /// 未知の mod 名は Err (旧来の GENERIC マーカー文字列書き込みは撤廃)。
    pub fn deploy_single_mod(mods_dir: &Path, mod_name: &str) -> Result<PathBuf, String> {
        fs::create_dir_all(mods_dir).map_err(|e| e.to_string())?;
        let payload = Self::mod_payload(mod_name)?;
        let dest = mods_dir.join(mod_name);
        Self::write_verified(&dest, payload, b"MZ")?;
        info!(
            "📦 [Self-Extract] Deployed embedded plugin payload {} ({} bytes) -> {:?}",
            mod_name,
            payload.len(),
            dest
        );
        Ok(dest)
    }

    /// すべての公式 `.dll` モッドを `.minecraft/mods/` へ展開する。
    /// 1 件でも埋め込み欠落/書き込み失敗があれば Err (黙ってスキップしない)。
    pub fn deploy_official_mods(mods_dir: &Path) -> Result<Vec<String>, String> {
        fs::create_dir_all(mods_dir).map_err(|e| e.to_string())?;
        let mut deployed = Vec::new();
        for name in ["rsgraphics.dll", "rscalc.dll", "rsreplay.dll"] {
            let payload = Self::mod_payload(name)?;
            let dest = mods_dir.join(name);
            Self::write_verified(&dest, payload, b"MZ")
                .map_err(|e| format!("deploy {}: {}", name, e))?;
            info!(
                "📦 [Self-Extract] Deployed self-contained official plugin {} ({} bytes) -> {:?}",
                name,
                payload.len(),
                dest
            );
            deployed.push(name.to_string());
        }
        Ok(deployed)
    }

    /// mod 名 → 埋め込み payload 解決 (未知名・未埋め込みは Err)。
    fn mod_payload(mod_name: &str) -> Result<&'static [u8], String> {
        let payload = match mod_name {
            "rsgraphics.dll" => PAYLOAD_RSGRAPHICS_DLL,
            "rscalc.dll" => PAYLOAD_RSCALC_DLL,
            "rsreplay.dll" => PAYLOAD_RSREPLAY_DLL,
            _ => return Err(format!("未知の組み込み mod 名: {}", mod_name)),
        };
        payload.ok_or_else(|| {
            format!(
                "{} payload 未埋め込み — `cargo build --release -p {}` 後に installer を再ビルドすること",
                mod_name,
                mod_name.trim_end_matches(".dll")
            )
        })
    }

    /// マジックバイト検証つき書き込み (埋め込みデータ破損の書き出しを防ぐ)。
    fn write_verified(dest: &Path, bytes: &[u8], magic: &[u8]) -> Result<(), String> {
        if !bytes.starts_with(magic) {
            return Err(format!(
                "埋め込み payload のマジック不一致 (期待 {:?}, 実際 {:?}): {:?} への書き込みを中止",
                magic,
                &bytes[..bytes.len().min(4)],
                dest
            ));
        }
        fs::write(dest, bytes).map_err(|e| format!("write {:?}: {}", dest, e))
    }
}
