//! # Rsift Rendering Extensions (Fabric Rendering API Parity & wgpu Integration)
//!
//! Fabric のクライアント描画 API を網羅し、純正の LWJGL を経由せずに
//! Rsift のゼロヒープ `wgpu` レンダリングエンジンにマップします。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, debug};

/// ブロックのレンダーレイヤータイプ (`BlockRenderLayerMap`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderLayer {
    Solid,
    Cutout,
    CutoutMipped,
    Translucent,
    Tripwire,
}

/// HUDレンダーコールバック (`HudRenderCallback`)
pub type HudRenderFn = Arc<dyn Fn(u32, u32, f32) + Send + Sync>; // width, height, tick_delta

/// スクリーン（GUI）描画・操作コールバック (`ScreenEvents`)
pub type ScreenInitFn = Arc<dyn Fn(&str, u32, u32) + Send + Sync>; // screen_title, width, height
pub type ScreenDrawFn = Arc<dyn Fn(&str, i32, i32, f32) + Send + Sync>; // title, mouse_x, mouse_y, delta

/// ブロックおよびアイテムのカラープロバイダー (`ColorProviderRegistry`)
pub type BlockColorFn = Arc<dyn Fn(i32, i32, i32, u32) -> u32 + Send + Sync>; // x, y, z, tint_index -> ARGB
pub type ItemColorFn = Arc<dyn Fn(u32, u32) -> u32 + Send + Sync>; // item_id, tint_index -> ARGB

/// モデル読み込み拡張 (`ModelLoadingRegistry` / `ModelAppender`)
pub type ModelAppenderFn = Arc<dyn Fn(&mut Vec<RegistryKey>) + Send + Sync>;

/// 統合レンダリングレジストリ
#[derive(Default, Clone)]
pub struct RenderingRegistry {
    pub entity_renderers: HashMap<RegistryKey, String>, // entity_key -> dll_shader_symbol
    pub block_entity_renderers: HashMap<RegistryKey, String>,
    pub block_render_layers: HashMap<RegistryKey, RenderLayer>,
    pub block_colors: HashMap<RegistryKey, BlockColorFn>,
    pub item_colors: HashMap<RegistryKey, ItemColorFn>,
    pub hud_callbacks: Vec<HudRenderFn>,
    pub screen_init_callbacks: Vec<ScreenInitFn>,
    pub screen_draw_callbacks: Vec<ScreenDrawFn>,
    pub model_appenders: Vec<ModelAppenderFn>,
    pub particle_factories: HashMap<RegistryKey, String>,
}

impl RenderingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// エンティティレンダラーを登録 (`EntityRendererRegistry.register`)
    pub fn register_entity_renderer(&mut self, entity_key: RegistryKey, shader_symbol: &str) {
        info!("Registering wgpu EntityRenderer for {}", entity_key.as_str());
        self.entity_renderers.insert(entity_key, shader_symbol.to_string());
        crate::platform::mark_dirty();
    }

    /// ブロックエンティティレンダラーを登録 (`BlockEntityRendererRegistry.register`)
    pub fn register_block_entity_renderer(&mut self, block_entity_key: RegistryKey, shader_symbol: &str) {
        info!("Registering wgpu BlockEntityRenderer for {}", block_entity_key.as_str());
        self.block_entity_renderers.insert(block_entity_key, shader_symbol.to_string());
        crate::platform::mark_dirty();
    }

    /// ブロックのレンダーレイヤーを設定 (`BlockRenderLayerMap.putBlock`)
    pub fn put_block_render_layer(&mut self, block_key: RegistryKey, layer: RenderLayer) {
        debug!("Setting RenderLayer for {} -> {:?}", block_key.as_str(), layer);
        self.block_render_layers.insert(block_key, layer);
        crate::platform::mark_dirty();
    }

    /// カラープロバイダーを登録 (`ColorProviderRegistry.BLOCK` / `.ITEM`)
    pub fn register_block_color_provider(&mut self, block_key: RegistryKey, provider: BlockColorFn) {
        self.block_colors.insert(block_key, provider);
        crate::platform::mark_dirty();
    }

    pub fn register_item_color_provider(&mut self, item_key: RegistryKey, provider: ItemColorFn) {
        self.item_colors.insert(item_key, provider);
        crate::platform::mark_dirty();
    }

    /// HUD描画フックを登録 (`HudRenderCallback.EVENT.register`)
    pub fn register_hud_callback(&mut self, cb: HudRenderFn) {
        self.hud_callbacks.push(cb);
        crate::platform::mark_dirty();
    }

    /// スクリーン描画イベントを登録 (`ScreenEvents.AFTER_INIT` / `AFTER_RENDER`)
    pub fn register_screen_events(&mut self, init_cb: ScreenInitFn, draw_cb: ScreenDrawFn) {
        self.screen_init_callbacks.push(init_cb);
        self.screen_draw_callbacks.push(draw_cb);
        crate::platform::mark_dirty();
    }

    /// モデルアペンダーを登録 (`ModelLoadingRegistry.registerAppender`)
    pub fn register_model_appender(&mut self, appender: ModelAppenderFn) {
        self.model_appenders.push(appender);
        crate::platform::mark_dirty();
    }

    /// パーティクルファクトリーを登録 (`ParticleFactoryRegistry.register`)
    pub fn register_particle_factory(&mut self, particle_key: RegistryKey, factory_symbol: &str) {
        self.particle_factories.insert(particle_key, factory_symbol.to_string());
        crate::platform::mark_dirty();
    }

    /// 毎フレームのHUDコールバックの実行（wgpu描画ループから呼ばれる）
    pub fn dispatch_hud(&self, width: u32, height: u32, delta_time: f32) {
        for cb in &self.hud_callbacks {
            cb(width, height, delta_time);
        }
    }
}
