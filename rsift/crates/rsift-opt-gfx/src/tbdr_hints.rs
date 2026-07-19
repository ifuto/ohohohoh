//! TBDR (Tile-Based Deferred Rendering) 向けヒント。
//!
//! Apple Silicon / Intel Iris / モバイル GPU はタイルベース。パス内でのみ使う
//! アタッチメントを `transient` + `LAZILY_ALLOCATED` にすると外部メモリ帯域を
//! 大幅削減できる（ARM のサンプルで 36–62% 削減報告あり）。

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

pub struct TbdrHints;
impl TbdrHints {
    pub fn wgsl_source(&self) -> &'static str {
        TBDR_HINTS_WGSL
    }
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
