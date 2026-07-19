
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
