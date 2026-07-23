//! Enhanced Barriersバッチ化 - D3D12 Enhanced Barriers モデル
//! 一次情報: Microsoft DirectX-Specs/D3D12EnhancedBarriers.md
//!
//! SyncBefore/SyncAfter・AccessBefore/AccessAfter・Layout の 3 軸分離を
//! モデル化する。2026-07-23 wave 54 で以下を厳格化:
//! - 旧実装の `transition` は sync を All/All にハードコードし「分割バリアで
//!   Upload と描画をオーバーラップ」と謳いながら、overlap を可能にする細粒度
//!   sync スコープを**表現不能**だった → 全パラメータ指定の `barrier()` を
//!   追加 (本モジュールの本来の表現)。`transition` は安全側既定 All/All の
//!   簡易 API として残す (既定が保守的なら粗いが誤りではない)。
//! - 仕様書の互換性表 (Layout-Access / Access-Sync / buffer-layout 排他) を
//!   `TextureBarrier::validate` として機械実装し、全構築経路で assert。
//! - 仕様上 `LAYOUT_PRESENT == LAYOUT_COMMON` (共に 0 のエイリアス) なので
//!   `Present` のアクセス互換は `Common` と同一として扱う。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierSync {
    None,
    All,
    Draw,
    Compute,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierAccess {
    NoAccess,
    Common,
    VertexBuffer,
    IndexBuffer,
    ConstantBuffer,
    ShaderResource,
    UnorderedAccess,
    RenderTarget,
    DepthWrite,
    CopySource,
    CopyDest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierLayout {
    Common,
    GenericRead,
    RenderTarget,
    UnorderedAccess,
    DepthWrite,
    CopySource,
    CopyDest,
    /// 仕様上は `D3D12_BARRIER_LAYOUT_COMMON` のエイリアス (同じ値 0)。
    /// アクセス互換は Common と同一として検証する。
    Present,
}

/// 全サブリソース指定 (transition 簡易 API で使用)。
pub const SUBRESOURCE_ALL: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone)]
pub struct TextureBarrier {
    pub resource_id: u32,
    pub sync_before: BarrierSync,
    pub sync_after: BarrierSync,
    pub access_before: BarrierAccess,
    pub access_after: BarrierAccess,
    pub layout_before: BarrierLayout,
    pub layout_after: BarrierLayout,
    pub subresource: u32,
}

impl TextureBarrier {
    /// レイアウトとアクセスの互換性 (D3D12EnhancedBarriers.md
    /// "Layout Access Compatibility" 表 + "buffer は layout を持たない"
    /// 原則を機械化)。`NoAccess` は任意レイアウトで許容 (no-claim マーカ、
    /// spec "Any access bits" の対称性と UNDEFINED 規則からの帰結)。
    /// `Common` アクセスは legacy 互換の包含として許容 (spec 表に per-layout
    /// 明細行は無し — モデル規則)。`Present` は `Common` のエイリアス。
    fn layout_access_ok(layout: BarrierLayout, access: BarrierAccess) -> bool {
        use BarrierAccess as A;
        use BarrierLayout as L;
        match access {
            A::NoAccess | A::Common => true,
            // buffer 専用 access は texture barrier ではカテゴリエラー
            // (spec: "Buffer resources have only a linear layout")
            A::VertexBuffer | A::IndexBuffer | A::ConstantBuffer => false,
            A::ShaderResource => matches!(layout, L::Common | L::Present | L::GenericRead),
            A::CopySource => matches!(
                layout,
                L::Common | L::Present | L::GenericRead | L::CopySource
            ),
            A::CopyDest => matches!(layout, L::Common | L::Present | L::CopyDest),
            A::RenderTarget => layout == L::RenderTarget,
            A::UnorderedAccess => layout == L::UnorderedAccess,
            A::DepthWrite => layout == L::DepthWrite,
        }
    }

    /// 同期スコープとアクセスの互換性 (spec "Access Bits Barrier Sync
    /// Compatibility" 表を、本モデルの sync enum 変種へ射影して機械化)。
    /// All は universal、`NoAccess`/`Common` は任意 sync で許容。
    /// `None` は「同期不要 = アクセス無し」のみとするモデル規則。
    fn sync_access_ok(sync: BarrierSync, access: BarrierAccess) -> bool {
        use BarrierAccess as A;
        use BarrierSync as S;
        match sync {
            S::None => access == A::NoAccess,
            S::All => true, // 全 access 行に ALL が含まれる (universal)
            S::Draw => matches!(
                access,
                A::NoAccess
                    | A::Common
                    | A::VertexBuffer
                    | A::IndexBuffer
                    | A::ConstantBuffer
                    | A::ShaderResource
                    | A::UnorderedAccess
                    | A::RenderTarget
                    | A::DepthWrite
            ),
            S::Compute => matches!(
                access,
                A::NoAccess
                    | A::Common
                    | A::ConstantBuffer
                    | A::ShaderResource
                    | A::UnorderedAccess
            ),
            S::Copy => matches!(
                access,
                A::NoAccess | A::Common | A::CopySource | A::CopyDest
            ),
        }
    }

    /// バリア全体の仕様適合性検証。違反理由を `Err` の静的文字列で返す。
    pub fn validate(&self) -> Result<(), &'static str> {
        for (side, sync, access, layout) in [
            (
                "before",
                self.sync_before,
                self.access_before,
                self.layout_before,
            ),
            (
                "after",
                self.sync_after,
                self.access_after,
                self.layout_after,
            ),
        ] {
            if !Self::sync_access_ok(sync, access) {
                return Err(match side {
                    "before" => "sync_before/access_before 不整合 (Access-Sync 互換性表違反)",
                    _ => "sync_after/access_after 不整合 (Access-Sync 互換性表違反)",
                });
            }
            if !Self::layout_access_ok(layout, access) {
                return Err(match side {
                    "before" => "layout_before/access_before 不整合 (Layout-Access 互換性表違反)",
                    _ => "layout_after/access_after 不整合 (Layout-Access 互換性表違反)",
                });
            }
        }
        Ok(())
    }
}

pub struct BarrierBatch {
    barriers: Vec<TextureBarrier>,
}

impl BarrierBatch {
    pub fn new() -> Self {
        Self {
            barriers: Vec::new(),
        }
    }

    /// 全パラメータ指定の本来 API (spec の TEXTURE_BARRIER に対応)。
    /// 仕様適合性を assert で機械強制する (fail-loud)。
    #[allow(clippy::too_many_arguments)]
    pub fn barrier(
        &mut self,
        id: u32,
        sync_before: BarrierSync,
        sync_after: BarrierSync,
        access_before: BarrierAccess,
        access_after: BarrierAccess,
        layout_before: BarrierLayout,
        layout_after: BarrierLayout,
        subresource: u32,
    ) {
        let b = TextureBarrier {
            resource_id: id,
            sync_before,
            sync_after,
            access_before,
            access_after,
            layout_before,
            layout_after,
            subresource,
        };
        assert!(b.validate().is_ok(), "違法バリア: {:?}", b.validate().err());
        self.barriers.push(b);
    }

    /// 簡易 API: sync は保守的既定の All/All (細粒度スコープが必要なら
    /// `barrier()` を使う)。全サブリソース指定。
    pub fn transition(
        &mut self,
        id: u32,
        before_layout: BarrierLayout,
        after_layout: BarrierLayout,
        before_access: BarrierAccess,
        after_access: BarrierAccess,
    ) {
        self.barrier(
            id,
            BarrierSync::All,
            BarrierSync::All,
            before_access,
            after_access,
            before_layout,
            after_layout,
            SUBRESOURCE_ALL,
        );
    }

    /// UAV バリア (同一 layout/access のまま sync のみ)。
    /// `subresource` を明示引数化 (旧実装は暗黙の 0 固定で transition の
    /// 全サブリソース規約と不整合だった — 呼出側が範囲を明示する)。
    pub fn uav_barrier(&mut self, id: u32, subresource: u32) {
        self.barrier(
            id,
            BarrierSync::Compute,
            BarrierSync::Compute,
            BarrierAccess::UnorderedAccess,
            BarrierAccess::UnorderedAccess,
            BarrierLayout::UnorderedAccess,
            BarrierLayout::UnorderedAccess,
            subresource,
        );
    }

    pub fn flush(&mut self) -> Vec<TextureBarrier> {
        std::mem::take(&mut self.barriers)
    }

    pub fn batch_size(&self) -> usize {
        self.barriers.len()
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn transition_fields_exact() {
        let mut b = BarrierBatch::new();
        assert_eq!(b.batch_size(), 0);
        b.transition(
            7,
            BarrierLayout::Present,
            BarrierLayout::RenderTarget,
            BarrierAccess::Common,
            BarrierAccess::RenderTarget,
        );
        assert_eq!(b.batch_size(), 1);
        let out = b.flush();
        let t = &out[0];
        assert_eq!(t.resource_id, 7);
        assert_eq!(
            t.sync_before,
            BarrierSync::All,
            "transition は Split ではなく All/All 固定"
        );
        assert_eq!(t.sync_after, BarrierSync::All);
        assert_eq!(t.access_before, BarrierAccess::Common);
        assert_eq!(t.access_after, BarrierAccess::RenderTarget);
        assert_eq!(t.layout_before, BarrierLayout::Present);
        assert_eq!(t.layout_after, BarrierLayout::RenderTarget);
        assert_eq!(t.subresource, SUBRESOURCE_ALL, "全サブリソース指定");
        assert_eq!(b.batch_size(), 0, "flush はキューを drain する");
    }

    #[test]
    fn uav_barrier_fields_exact() {
        let mut b = BarrierBatch::new();
        b.uav_barrier(3, 0);
        let out = b.flush();
        let t = &out[0];
        assert_eq!(t.resource_id, 3);
        assert_eq!(t.sync_before, BarrierSync::Compute);
        assert_eq!(t.sync_after, BarrierSync::Compute);
        assert_eq!(t.access_before, BarrierAccess::UnorderedAccess);
        assert_eq!(t.access_after, BarrierAccess::UnorderedAccess);
        assert_eq!(t.layout_before, BarrierLayout::UnorderedAccess);
        assert_eq!(t.layout_after, BarrierLayout::UnorderedAccess);
        assert_eq!(t.subresource, 0, "subresource は明示引数の値を保持");
        // 明示引数は全サブリソース指定も可能
        let mut b = BarrierBatch::new();
        b.uav_barrier(4, SUBRESOURCE_ALL);
        assert_eq!(b.flush()[0].subresource, SUBRESOURCE_ALL);
    }

    #[test]
    fn flush_preserves_insertion_order_and_empties() {
        let mut b = BarrierBatch::new();
        b.transition(
            10,
            BarrierLayout::Common,
            BarrierLayout::CopyDest,
            BarrierAccess::Common,
            BarrierAccess::CopyDest,
        );
        b.uav_barrier(20, 0);
        b.transition(
            30,
            BarrierLayout::CopyDest,
            BarrierLayout::Present,
            BarrierAccess::CopyDest,
            BarrierAccess::Common,
        );
        assert_eq!(b.batch_size(), 3);
        let out = b.flush();
        assert_eq!(
            out.iter().map(|t| t.resource_id).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        assert_eq!(b.batch_size(), 0);
        assert!(b.flush().is_empty(), "空に対する二重 flush は空 Vec");
    }

    /// wave 54 BD-1: 全パラメータ指定 API のフィールド完全保持 + validate 通過。
    #[test]
    fn barrier_full_params_exact() {
        let mut b = BarrierBatch::new();
        // spec 適合の細粒度スコープ例: Copy → ShaderResource (upload 読出し)
        b.barrier(
            5,
            BarrierSync::Copy,
            BarrierSync::Draw,
            BarrierAccess::CopyDest,
            BarrierAccess::ShaderResource,
            BarrierLayout::CopyDest,
            BarrierLayout::GenericRead,
            2,
        );
        let t = &b.flush()[0];
        assert_eq!(t.sync_before, BarrierSync::Copy);
        assert_eq!(t.sync_after, BarrierSync::Draw);
        assert_eq!(t.access_before, BarrierAccess::CopyDest);
        assert_eq!(t.access_after, BarrierAccess::ShaderResource);
        assert_eq!(t.layout_before, BarrierLayout::CopyDest);
        assert_eq!(t.layout_after, BarrierLayout::GenericRead);
        assert_eq!(t.subresource, 2);
        assert!(t.validate().is_ok());
    }

    /// wave 54 BD-2: Layout-Access 互換性表 (spec 一次情報) の機械ピン。
    /// (layout, access) → 適合か
    #[test]
    fn validate_layout_access_table_exact() {
        use BarrierAccess as A;
        use BarrierLayout as L;
        let cases: &[(L, A, bool)] = &[
            (L::Common, A::ShaderResource, true),
            (L::Common, A::CopySource, true),
            (L::Common, A::CopyDest, true),
            (L::Present, A::CopyDest, true), // Present ≡ Common エイリアス
            (L::Common, A::RenderTarget, false),
            (L::Common, A::UnorderedAccess, false),
            (L::GenericRead, A::ShaderResource, true),
            (L::GenericRead, A::CopySource, true),
            (L::GenericRead, A::CopyDest, false),
            (L::RenderTarget, A::RenderTarget, true),
            (L::RenderTarget, A::ShaderResource, false),
            (L::UnorderedAccess, A::UnorderedAccess, true),
            (L::UnorderedAccess, A::ShaderResource, false),
            (L::DepthWrite, A::DepthWrite, true),
            (L::DepthWrite, A::ShaderResource, false),
            (L::CopySource, A::CopySource, true),
            (L::CopySource, A::CopyDest, false),
            (L::CopyDest, A::CopyDest, true),
            (L::CopyDest, A::CopySource, false),
        ];
        for &(l, a, ok) in cases {
            let b = TextureBarrier {
                resource_id: 0,
                sync_before: BarrierSync::All,
                sync_after: BarrierSync::All,
                access_before: A::NoAccess,
                access_after: a,
                layout_before: L::Common,
                layout_after: l,
                subresource: 0,
            };
            assert_eq!(
                b.validate().is_ok(),
                ok,
                "layout={l:?} access={a:?} の適合判定が spec 表と不一致"
            );
        }
        // NoAccess / Common は任意レイアウトで許容
        for l in [
            L::Common,
            L::GenericRead,
            L::RenderTarget,
            L::UnorderedAccess,
            L::DepthWrite,
            L::CopySource,
            L::CopyDest,
            L::Present,
        ] {
            for a in [A::NoAccess, A::Common] {
                assert!(
                    TextureBarrier {
                        resource_id: 0,
                        sync_before: BarrierSync::All,
                        sync_after: BarrierSync::All,
                        access_before: A::NoAccess,
                        access_after: a,
                        layout_before: L::Common,
                        layout_after: l,
                        subresource: 0,
                    }
                    .validate()
                    .is_ok(),
                    "{l:?}/{a:?} は許容されるべき"
                );
            }
        }
    }

    /// wave 54 BD-2: buffer 専用 access は texture barrier ではカテゴリエラー
    /// (spec: "Buffer resources have only a linear layout")。
    #[test]
    fn validate_rejects_buffer_access_in_texture_barrier() {
        for a in [
            BarrierAccess::VertexBuffer,
            BarrierAccess::IndexBuffer,
            BarrierAccess::ConstantBuffer,
        ] {
            let b = TextureBarrier {
                resource_id: 0,
                sync_before: BarrierSync::All,
                sync_after: BarrierSync::All,
                access_before: BarrierAccess::NoAccess,
                access_after: a,
                layout_before: BarrierLayout::Common,
                layout_after: BarrierLayout::Common,
                subresource: 0,
            };
            assert!(b.validate().is_err(), "{a:?} は texture barrier で不適合");
        }
    }

    /// wave 54 BD-2: Access-Sync 互換性表 (本モデル sync 変種への射影)。
    #[test]
    fn validate_sync_access_table_exact() {
        use BarrierAccess as A;
        use BarrierSync as S;
        let cases: &[(S, A, bool)] = &[
            (S::All, A::RenderTarget, true),
            (S::All, A::CopyDest, true),
            (S::Draw, A::RenderTarget, true),
            (S::Draw, A::ShaderResource, true),
            (S::Draw, A::DepthWrite, true),
            (S::Draw, A::CopyDest, false), // COPY は ALL/COPY のみ
            (S::Draw, A::CopySource, false),
            (S::Compute, A::UnorderedAccess, true),
            // ConstantBuffer は buffer 専用 access で texture barrier の評価域外
            // (validate_rejects_buffer_access_in_texture_barrier で別途ピン)
            (S::Compute, A::ShaderResource, true),
            (S::Compute, A::VertexBuffer, false),
            (S::Compute, A::RenderTarget, false),
            (S::Copy, A::CopyDest, true),
            (S::Copy, A::CopySource, true),
            (S::Copy, A::ShaderResource, false),
            (S::None, A::NoAccess, true),
            (S::None, A::ShaderResource, false),
        ];
        for &(s, a, ok) in cases {
            // sync 規則のみを分離評価するため、access に既知適合な layout を
            // 常に選ぶ (layout 表は別テストで検証済)。
            let layout = match a {
                A::RenderTarget => BarrierLayout::RenderTarget,
                A::UnorderedAccess => BarrierLayout::UnorderedAccess,
                A::DepthWrite => BarrierLayout::DepthWrite,
                A::CopySource => BarrierLayout::CopySource,
                A::CopyDest => BarrierLayout::CopyDest,
                // NoAccess/Common/ShaderResource/その他は Common が適合
                _ => BarrierLayout::Common,
            };
            let b = TextureBarrier {
                resource_id: 0,
                sync_before: S::All,
                sync_after: s,
                access_before: A::NoAccess,
                access_after: a,
                layout_before: BarrierLayout::Common,
                layout_after: layout,
                subresource: 0,
            };
            assert_eq!(b.validate().is_ok(), ok, "sync={s:?} access={a:?}");
        }
    }

    /// wave 54: 違法組合せは構築経路で fail-loud。
    #[test]
    fn barrier_rejects_illegal_combo() {
        let r = std::panic::catch_unwind(|| {
            let mut b = BarrierBatch::new();
            // Copy sync に ShaderResource access は表違反
            b.barrier(
                1,
                BarrierSync::Copy,
                BarrierSync::Copy,
                BarrierAccess::ShaderResource,
                BarrierAccess::ShaderResource,
                BarrierLayout::Common,
                BarrierLayout::Common,
                0,
            );
        });
        assert!(r.is_err());
        let r = std::panic::catch_unwind(|| {
            let mut b = BarrierBatch::new();
            // RenderTarget アクセスに Common レイアウトは表違反
            b.transition(
                1,
                BarrierLayout::Common,
                BarrierLayout::Common,
                BarrierAccess::NoAccess,
                BarrierAccess::RenderTarget,
            );
        });
        assert!(r.is_err());
    }
}
