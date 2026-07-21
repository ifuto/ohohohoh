
//! Enhanced Barriersバッチ化 - D3D12 Enhanced Barriers仕様に基づく分割バリアでUploadと描画をオーバーラップ
//! Microsoft DirectX-Specs/D3D12EnhancedBarriers.mdより: SyncBefore/SyncAfter、AccessBefore/AccessAfter、Layout分離

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
    Present,
}

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

pub struct BarrierBatch {
    barriers: Vec<TextureBarrier>,
}

impl BarrierBatch {
    pub fn new() -> Self { Self { barriers: Vec::new() } }

    pub fn transition(&mut self, id: u32, before_layout: BarrierLayout, after_layout: BarrierLayout, before_access: BarrierAccess, after_access: BarrierAccess) {
        self.barriers.push(TextureBarrier {
            resource_id: id,
            sync_before: BarrierSync::All,
            sync_after: BarrierSync::All,
            access_before: before_access,
            access_after: after_access,
            layout_before: before_layout,
            layout_after: after_layout,
            subresource: 0xFFFFFFFF,
        });
    }

    pub fn uav_barrier(&mut self, id: u32) {
        self.barriers.push(TextureBarrier {
            resource_id: id,
            sync_before: BarrierSync::Compute,
            sync_after: BarrierSync::Compute,
            access_before: BarrierAccess::UnorderedAccess,
            access_after: BarrierAccess::UnorderedAccess,
            layout_before: BarrierLayout::UnorderedAccess,
            layout_after: BarrierLayout::UnorderedAccess,
            subresource: 0,
        });
    }

    pub fn flush(&mut self) -> Vec<TextureBarrier> {
        std::mem::take(&mut self.barriers)
    }

    pub fn batch_size(&self) -> usize { self.barriers.len() }
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
        assert_eq!(t.sync_before, BarrierSync::All, "transition は Split ではなく All/All 固定");
        assert_eq!(t.sync_after, BarrierSync::All);
        assert_eq!(t.access_before, BarrierAccess::Common);
        assert_eq!(t.access_after, BarrierAccess::RenderTarget);
        assert_eq!(t.layout_before, BarrierLayout::Present);
        assert_eq!(t.layout_after, BarrierLayout::RenderTarget);
        assert_eq!(t.subresource, 0xFFFF_FFFF, "全サブリソース指定");
        assert_eq!(b.batch_size(), 0, "flush はキューを drain する");
    }

    #[test]
    fn uav_barrier_fields_exact() {
        let mut b = BarrierBatch::new();
        b.uav_barrier(3);
        let out = b.flush();
        let t = &out[0];
        assert_eq!(t.resource_id, 3);
        assert_eq!(t.sync_before, BarrierSync::Compute);
        assert_eq!(t.sync_after, BarrierSync::Compute);
        assert_eq!(t.access_before, BarrierAccess::UnorderedAccess);
        assert_eq!(t.access_after, BarrierAccess::UnorderedAccess);
        assert_eq!(t.layout_before, BarrierLayout::UnorderedAccess);
        assert_eq!(t.layout_after, BarrierLayout::UnorderedAccess);
        assert_eq!(t.subresource, 0, "uav_barrier は subresource 0 固定 (transition の全指定と異なる)");
    }

    #[test]
    fn flush_preserves_insertion_order_and_empties() {
        let mut b = BarrierBatch::new();
        b.transition(10, BarrierLayout::Common, BarrierLayout::CopyDest, BarrierAccess::Common, BarrierAccess::CopyDest);
        b.uav_barrier(20);
        b.transition(30, BarrierLayout::CopyDest, BarrierLayout::Present, BarrierAccess::CopyDest, BarrierAccess::Common);
        assert_eq!(b.batch_size(), 3);
        let out = b.flush();
        assert_eq!(out.iter().map(|t| t.resource_id).collect::<Vec<_>>(), vec![10, 20, 30]);
        assert_eq!(b.batch_size(), 0);
        assert!(b.flush().is_empty(), "空に対する二重 flush は空 Vec");
    }
}
