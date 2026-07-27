//! TBDR (Tile-Based Deferred Rendering) 向けヒント。
//!
//! Apple Silicon / Intel Iris / モバイル GPU はタイルベース。パス内でのみ使う
//! アタッチメントを `transient` + `LAZILY_ALLOCATED` にすると外部メモリ帯域を
//! 大幅削減できる（ARM のサンプルで 36–62% 削減報告あり）。
//!
//! 誠実注記 (wave 138 EL-3):
//! 1. 判定は **conservative**: 「パス内書込み && 外部読取なし」のみを
//!    Transient とする。TBDR 理論上は load-op clear のみで内容を読まない
//!    アタッチメント (writes=false) も transient 化しうるが、現契約は
//!    非対象 (全 Persistent = 安全側)。
//! 2. wiring (full_graph_wiring) での消費は初期化時の固定 2 パターン定数入力
//!    (shadow: writes/!reads、scene: writes/reads) の info! ログ評価であり、
//!    実レンダーパス構造との動的接続はない (畳み込み可能なハードコード契約)。
//! 3. `TBDR_HINTS_WGSL` は GPU シェーダーではない (ファイル自身が
//!    "No GPU shader required" と宣言) にも関わらず
//!    `gpu_runtime::all_wgsl_sources` (「全 Wave モジュールの実シェーダー」
//!    一覧) に登録されている。空コメントのみのモジュールは naga で受理
//!    されるため実質の検証対象は存在しない (分類実態の公表。一覧からの
//!    除去は naga 検証経路と wiring 連結 pin への波及を伴うため設計引継ぎ)。
//! 4. `lazy_allocated() == (recommended_usage() == Transient)` は定義同値の
//!    委托であり独立の判定ではない。

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttachmentUsage {
    Transient,
    Persistent,
}

/// 1 つのレンダーパスにおけるアタッチメントの使用パターン。
pub struct TbdrPass {
    /// パス内で書き込まれるか。
    pub writes: bool,
    /// 別のパス（後段）からも読まれるか。
    pub reads_outside_pass: bool,
}

impl TbdrPass {
    /// パス内でのみ書かれ、外部で読まれないなら transient にマーク。
    pub fn recommended_usage(&self) -> AttachmentUsage {
        if self.writes && !self.reads_outside_pass {
            AttachmentUsage::Transient
        } else {
            AttachmentUsage::Persistent
        }
    }

    /// transient なら遅延確保（LAZILY_ALLOCATED）が推奨。
    pub fn lazy_allocated(&self) -> bool {
        self.recommended_usage() == AttachmentUsage::Transient
    }
}

/// WGSL 取得の唯一の公式アクセスポイント (wave 138 EL-1)。
///
/// 旧 `TbdrHints` unit struct ラッパ (`wgsl_source(&self)`) は crate 全体で
/// 消費者完全ゼロ (新指令 §7「消費者なし禁止」違反) で、状態を持たず
/// `&self` を使わない装飾メソッドだったため削除 (EJ-2 Vec3/Vec4 完全装飾
/// 削除と同型)。ssr/bloom/cas 他 20+ モジュールと同じ free fn 様式に統一し、
/// gpu_runtime::all_wgsl_sources の登録を本関数経由に一本化した。
pub fn wgsl_source() -> &'static str {
    TBDR_HINTS_WGSL
}
pub const TBDR_HINTS_WGSL: &str = include_str!("../shaders/tbdr_hints.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_when_pass_local() {
        let p = TbdrPass {
            writes: true,
            reads_outside_pass: false,
        };
        assert_eq!(p.recommended_usage(), AttachmentUsage::Transient);
        assert!(p.lazy_allocated());
    }

    #[test]
    fn persistent_when_read_later() {
        let p = TbdrPass {
            writes: true,
            reads_outside_pass: true,
        };
        assert_eq!(p.recommended_usage(), AttachmentUsage::Persistent);
        assert!(!p.lazy_allocated());
    }

    #[test]
    fn not_written_is_persistent() {
        let p = TbdrPass {
            writes: false,
            reads_outside_pass: false,
        };
        assert_eq!(p.recommended_usage(), AttachmentUsage::Persistent);
        assert!(!p.lazy_allocated());
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EL-2: 真理値表 exhaustive — 4 行すべて機械列挙。
    /// 旧テスト群は (T,F)/(T,T)/(F,F) の 3 行のみで (F,T) 行が検出空白
    /// だった (rq 事前導出: or 変異は (T,T)(F,F) の 2 行差異・否定脱落
    /// 変異は (T,F)(F,T) の 2 行差異 → 4 行網羅で両変異を捕捉可能)。
    #[test]
    fn truth_table_exhaustive() {
        for writes in [false, true] {
            for reads in [false, true] {
                let p = TbdrPass {
                    writes,
                    reads_outside_pass: reads,
                };
                let expect = if writes && !reads {
                    AttachmentUsage::Transient
                } else {
                    AttachmentUsage::Persistent
                };
                assert_eq!(
                    p.recommended_usage(),
                    expect,
                    "writes={writes} reads={reads}"
                );
                assert_eq!(p.lazy_allocated(), expect == AttachmentUsage::Transient);
            }
        }
    }

    /// EL-1: free fn は公開 const と同一内容 (TbdrHints ラッパ削除の等価 pin)。
    /// WGSL は GPU シェーダーではない CPU ヒント注記 (EL-3-3) なので
    /// naga 検証に代わる内容 pin をここで担保する。
    #[test]
    fn wgsl_source_identity_with_const() {
        assert_eq!(wgsl_source(), TBDR_HINTS_WGSL);
        assert!(wgsl_source().contains("transient"));
        assert!(wgsl_source().contains("No GPU shader required"));
    }
}
