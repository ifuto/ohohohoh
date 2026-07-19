//! # Texture & Image Embedding API (`image_api`)
//!
//! PNG/JPEG/raw RGBA bytes are stored and flushed to Minecraft `DynamicTexture`
//! via `RsiftPlatformBridge` (JNI). Draw calls become GuiGraphics blit ops.

use crate::platform::{global_images, store_image_bytes, mark_dirty, request_open_screen};
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug};

/// 埋め込み画像のメタデータ
#[derive(Debug, Clone)]
pub struct EmbeddedImage {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub gpu_texture_bindless_id: u32,
}

/// 画像埋め込み＆テクスチャレジストリ
#[derive(Default, Clone)]
pub struct ImageRegistry {
    pub images: Arc<RwLock<HashMap<u32, EmbeddedImage>>>,
    pub next_id: Arc<RwLock<u32>>,
}

impl ImageRegistry {
    pub fn new() -> Self {
        Self {
            images: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(50000)),
        }
    }

    /// Embed RGBA (or raw image bytes) and schedule DynamicTexture upload on the JVM.
    pub fn embed_from_bytes(&self, name: &str, raw_rgba: &[u8], width: u32, height: u32) -> u32 {
        let mut id_guard = self.next_id.write().unwrap();
        let id = *id_guard;
        *id_guard += 1;

        let bindless_id = id;
        info!(
            "[ImageRegistry] Embedded texture [{}] ({}x{}) -> GPU ID #{}",
            name, width, height, bindless_id
        );

        let img = EmbeddedImage {
            id,
            name: name.to_string(),
            width,
            height,
            gpu_texture_bindless_id: bindless_id,
        };

        if let Ok(mut map) = self.images.write() {
            map.insert(id, img);
        }

        // Keep bytes for JNI DynamicTexture upload.
        let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
        let mut rgba = raw_rgba.to_vec();
        if rgba.len() < expected {
            rgba.resize(expected, 255);
        } else if rgba.len() > expected && expected > 0 {
            rgba.truncate(expected);
        }
        store_image_bytes(id, rgba);

        // Mirror into global registry used by platform snapshot.
        let global = global_images();
        if let Ok(mut map) = global.images.write() {
            map.insert(
                id,
                EmbeddedImage {
                    id,
                    name: name.to_string(),
                    width,
                    height,
                    gpu_texture_bindless_id: bindless_id,
                },
            );
        }
        if let Ok(mut g) = global.next_id.write() {
            *g = (*g).max(id + 1);
        }

        mark_dirty();
        id
    }

    /// Queue a draw of the texture on the next platform UI pass.
    pub fn draw_texture(&self, texture_id: u32, x: i32, y: i32, render_width: u32, render_height: u32) {
        if let Ok(map) = self.images.read() {
            if let Some(img) = map.get(&texture_id) {
                debug!(
                    "[ImageRegistry] Draw [{}] GPU#{} at ({},{}) {}x{}",
                    img.name, img.gpu_texture_bindless_id, x, y, render_width, render_height
                );
            }
        }
        // Encode draw request as a pending screen hint consumed by platform bridge.
        request_open_screen(&format!(
            "draw_tex:{}:{}:{}:{}:{}",
            texture_id, x, y, render_width, render_height
        ));
    }
}
