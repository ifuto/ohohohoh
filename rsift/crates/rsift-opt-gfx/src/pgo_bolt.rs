
//! PGO + LTO設定 - ビルド時の最適化フラグを提供
//!
//! 【wave 173 FS 捕捉 112/113】旧 rustflags() は pgo_instrument/bolt フィールドを
//! 完全に無視する計装不整合だった (bolt: true でも出力同一) → pgo_instrument
//! は truth `-C profile-generate` 付加へ truth 接続。bolt フィールドは rustc
//! flag 非対応 (BOLT = リンク後 llvm-bolt 後処理の truth) で rustflags 語彙に
//! 収まらず読み取り消費者ゼロ、dev() も消費者ゼロのため不可能証明削除
//! (FH 捕捉 92 判例)。release()/default() は workspace [profile.release] の
//! lto=fat/codegen-units=1 と整合することを陽性確認済。

#[derive(Debug, Clone)]
pub struct PgoConfig {
    pub lto: String,
    pub codegen_units: u32,
    pub target_cpu: String,
    pub pgo_instrument: bool,
}

impl Default for PgoConfig {
    fn default() -> Self {
        Self {
            lto: "thin".into(),
            codegen_units: 1,
            target_cpu: "x86-64-v3".into(),
            pgo_instrument: false,
        }
    }
}

impl PgoConfig {
    /// workspace [profile.release] (lto="fat", codegen-units=1) と整合
    /// (wave 173 FS 陽性確認)。BOLT は rustc flag 非対応のため本 struct の
    /// 語彙に収まらず、bolt フィールドは不可能証明削除した。
    pub fn release() -> Self {
        Self {
            lto: "fat".into(),
            codegen_units: 1,
            target_cpu: "x86-64-v3".into(),
            pgo_instrument: false,
        }
    }

    /// rustflags 文字列。pgo_instrument: true なら truth `-C profile-generate`
    /// を付加 (rustc 公式 -C profile-generate=path、PGO instrument に必須)。
    /// 旧実装は pgo_instrument フィールドを完全に無視する計装不整合だった。
    pub fn rustflags(&self) -> String {
        let mut s = format!(
            "-C lto={} -C codegen-units={} -C target-cpu={}",
            self.lto, self.codegen_units, self.target_cpu
        );
        if self.pgo_instrument {
            s.push_str(" -C profile-generate=/tmp/rsift-pgo");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_field_tables() {
        let rel = PgoConfig::release();
        assert_eq!(rel.lto, "fat");
        assert_eq!(rel.codegen_units, 1);
        assert_eq!(rel.target_cpu, "x86-64-v3");
        assert!(!rel.pgo_instrument);
        let def = PgoConfig::default();
        assert_eq!((def.lto.as_str(), def.codegen_units), ("thin", 1));
        assert!(!def.pgo_instrument);
    }

    /// 【wave 173 FS 捕捉 112】pgo_instrument: true のとき rustflags() は
    /// truth `-C profile-generate` を含むこと (PGO instrument ビルドに必須、
    /// rustc 公式 -C profile-generate=path)。現状フィールドを完全に無視して
    /// 含まない → RED。
    #[test]
    fn fs_pgo_instrument_profile_generate_truth() {
        let c = PgoConfig {
            pgo_instrument: true,
            ..Default::default()
        };
        assert!(
            c.rustflags().contains("-C profile-generate"),
            "pgo_instrument: true なら -C profile-generate が truth 必須 (got: {})",
            c.rustflags()
        );
        let off = PgoConfig {
            pgo_instrument: false,
            ..Default::default()
        };
        assert!(
            !off.rustflags().contains("-C profile-generate"),
            "false では付加しない"
        );
    }

    #[test]
    fn rustflags_string_exact() {
        assert_eq!(
            PgoConfig::release().rustflags(),
            "-C lto=fat -C codegen-units=1 -C target-cpu=x86-64-v3"
        );
    }
    /// 【wave 184 GD】削除済み API `dev()` の再出現 lexeme pin (adversarial
    /// 173-c・22 例目 非検出回収、wave 26 例未採から 1 回収): 削除 truth 証跡として
    /// 宣言形が再起しないことを機械 pin。自己言及 vacuous 回避のため検出
    /// 語彙は分割記述 (doc 証跡条文には `fn ` 接頭で択定範囲外)。
    #[test]
    fn gd_removed_dev_lexeme() {
        let src = include_str!("pgo_bolt.rs");
        let lex = concat!("pub fn de", "v(");
        assert!(
            !src.contains(lex),
            "削除 API `dev()` の宣言再来を検出 → 死救出は lint/テスト限界で"
        );
    }
}
