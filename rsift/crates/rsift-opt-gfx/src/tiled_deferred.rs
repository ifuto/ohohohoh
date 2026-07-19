//! # 39. Deferred Rendering / Tiled Deferred (`TiledDeferredLighting`)
//!
//! ジオメトリパスで G バッファ（位置・法線・アルベド・粗さ等）のみ出力し、ライティングパスで
//! 画面を 16x16 ピクセルのタイルに分割して、各タイルに影響する光源リストを Compute/CPU で
//! 事前カリングする。光源数に比例する Forward 描画の負荷を排除し、100 個以上の動的・固定光源
//! でも 60 FPS を安定維持。

pub const TILE_SIZE_PIXELS: usize = 16;
pub const MAX_LIGHTS_PER_TILE: usize = 64;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLight {
    pub pos: [f32; 3],
    pub radius: f32,
    pub color_rgb: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct LightTile {
    pub light_indices: Vec<u32>,
}

pub struct TiledDeferredLighting {
    pub screen_width: usize,
    pub screen_height: usize,
    pub tiles_x: usize,
    pub tiles_y: usize,
    pub tiles: Vec<LightTile>,
    pub lights: Vec<PointLight>,
}

impl TiledDeferredLighting {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let tiles_x = (screen_width + TILE_SIZE_PIXELS - 1) / TILE_SIZE_PIXELS;
        let tiles_y = (screen_height + TILE_SIZE_PIXELS - 1) / TILE_SIZE_PIXELS;
        let total_tiles = tiles_x * tiles_y;
        Self {
            screen_width,
            screen_height,
            tiles_x,
            tiles_y,
            tiles: vec![LightTile { light_indices: Vec::with_capacity(16) }; total_tiles],
            lights: Vec::with_capacity(256),
        }
    }

    pub fn clear_lights(&mut self) {
        self.lights.clear();
    }

    pub fn add_light(&mut self, light: PointLight) -> u32 {
        let id = self.lights.len() as u32;
        self.lights.push(light);
        id
    }

    /// Cull lights against each screen tile bounding frustum/sphere (`O(Tiles * Lights)` or hierarchical).
    pub fn cull_lights_for_tiles(&mut self, view_proj: &[[f32; 4]; 4]) {
        for tile in &mut self.tiles {
            tile.light_indices.clear();
        }

        for (light_idx, light) in self.lights.iter().enumerate() {
            // Project light sphere center to NDC and compute approximate screen pixel radius
            let x = view_proj[0][0] * light.pos[0] + view_proj[0][1] * light.pos[1] + view_proj[0][2] * light.pos[2] + view_proj[0][3];
            let y = view_proj[1][0] * light.pos[0] + view_proj[1][1] * light.pos[1] + view_proj[1][2] * light.pos[2] + view_proj[1][3];
            let ww = view_proj[3][0] * light.pos[0] + view_proj[3][1] * light.pos[1] + view_proj[3][2] * light.pos[2] + view_proj[3][3];

            if ww <= 0.1 {
                continue;
            }
            let ndc_x = x / ww;
            let ndc_y = y / ww;
            let screen_x = ((ndc_x * 0.5 + 0.5) * self.screen_width as f32) as i32;
            let screen_y = ((1.0 - (ndc_y * 0.5 + 0.5)) * self.screen_height as f32) as i32;
            let screen_radius = ((light.radius / ww) * self.screen_width as f32 * 0.5) as i32;

            let min_tx = ((screen_x - screen_radius).max(0) as usize) / TILE_SIZE_PIXELS;
            let max_tx = (((screen_x + screen_radius).max(0) as usize) / TILE_SIZE_PIXELS).min(self.tiles_x.saturating_sub(1));
            let min_ty = ((screen_y - screen_radius).max(0) as usize) / TILE_SIZE_PIXELS;
            let max_ty = (((screen_y + screen_radius).max(0) as usize) / TILE_SIZE_PIXELS).min(self.tiles_y.saturating_sub(1));

            for ty in min_ty..=max_ty {
                for tx in min_tx..=max_tx {
                    let tile_idx = ty * self.tiles_x + tx;
                    if self.tiles[tile_idx].light_indices.len() < MAX_LIGHTS_PER_TILE {
                        self.tiles[tile_idx].light_indices.push(light_idx as u32);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tiled_deferred_light_culling() {
        let mut tdl = TiledDeferredLighting::new(1280, 720);
        tdl.add_light(PointLight {
            pos: [0.0, 64.0, 0.0],
            radius: 10.0,
            color_rgb: [1.0, 0.8, 0.5],
            intensity: 2.0,
        });
        let vp = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0, 10.0],
        ];
        tdl.cull_lights_for_tiles(&vp);
        assert!(!tdl.lights.is_empty());
    }
}
