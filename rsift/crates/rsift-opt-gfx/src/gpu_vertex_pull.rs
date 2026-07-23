//! GPU vertex-pull 語彙 — SSBO quads, draw without index buffer。
//!
//! **live 描画パス**: `frame_pipeline` が `terrain_vertex_pull.wgsl`
//! (`SHADER_VERTEX_PULL`) を深度 (Depth32Float) 付き HDR (`Rgba16Float`)
//! ターゲットに実描画する (深度対応パイプラインは frame_pipeline 側 — 同
//! モジュール doc 参照)。本モジュールは両者が共有する**シェーダ語彙と
//! uniform レイアウトの単一供給元**。
//!
//! Meshlet カリング: `terrain_mesh_shader.wgsl` の compute エミュレーションを
//! `frame_worldgen::GpuMeshletCull` が実 dispatch する (mesh shader 自体は
//! WGSL 非対応のためソフトエミュレーションが正)。
//!
//! ## wave 58 BH 監査で撤去した死に構造 (2026-07-23、撤去根拠の記録)
//! - `GpuVertexPullEngine` / `draw_pull_mesh` (深度無し・サーフェス直結・
//!   draw 毎に SSBO を新規生成する単純パス): workspace 全域で**構築箇所
//!   ゼロ** (live 描画は frame_pipeline が深度対応パイプラインを別建てで
//!   実施 — frame_pipeline.rs :7,:245 に設計選択の記録あり)。実機 GPU で
//!   検証不能な複製エンジンを温存することは「このパスが動く」という嘘の
//!   維持に等しいため撤去 (BA-2 GpuUploadHeader 撤去と同根拠)。
//! - `PullEngineHandle`: 上記エンジンの lazy 保持ラッパ。構築箇所ゼロ。
//! - `PullSsboPool` / `PullPoolSlot` (render_pipeline :113,:232,:429):
//!   upload の戻り slot は呼出側で即破棄、`slots`/`generation` の reader は
//!   皆無の **write-only 帳簿** (`adaptive()` が HW プローブまで実行する死に
//!   重さ)。しかも 2 つの潜在バグを抱えていた: 容量超過時に stale slot を
//!   残したまま None を返す (旧メッシュへの静寂バージョンスキュー) 設計、
//!   `len as u32` の暗黙切捨て。消費者が存在しないため wiring ではなく撤去
//!   (将来 pooled ring SSBO を入れる場合は frame 単位の fence 設計と実機
//!   GPU 検証が前提 — ring wrap で同一フレーム先行チャンクを上書きする
//!   ハザードがあるため draw 毎新規 SSBO とは互換性が無い)。

pub const SHADER_VERTEX_PULL: &str = include_str!("../shaders/terrain_vertex_pull.wgsl");
pub const SHADER_MESH_SHADER: &str = include_str!("../shaders/terrain_mesh_shader.wgsl");

/// frame_pipeline が group(0) binding(0) にバインドする per-frame uniform。
/// WGSL `struct FrameUniforms` (mat4x4 64B + vec4 16B) とレイアウト一致が
/// 契約 (テストで厳密ピン)。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub chunk_origin: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::{demo_palette, mesh_section_pull};

    #[test]
    fn pull_mesh_no_indices() {
        let p = demo_palette(1, 2);
        let mesh = mesh_section_pull(&p, 1, 2);
        assert!(!mesh.is_empty() || mesh.quads.is_empty());
        assert_eq!(mesh.pull_vertex_count(), mesh.quads.len() as u32 * 6);
        if !mesh.quads.is_empty() {
            assert!(mesh.vram_ratio_vs_12b_indexed() > 2.0);
        }
    }

    /// wave 58 BH-4: FrameUniforms の GPU バインド契約の厳密ピン。
    /// WGSL struct (mat4x4<f32> 64B + vec4<f32> 16B = 80B) と一致。
    #[test]
    fn frame_uniforms_layout_matches_wgsl() {
        assert_eq!(
            std::mem::size_of::<FrameUniforms>(),
            80,
            "view_proj 64B + chunk_origin 16B = 80B (WGSL struct と一致)"
        );
        assert_eq!(std::mem::align_of::<FrameUniforms>(), 4);
        assert_eq!(
            std::mem::size_of::<FrameUniforms>() % 16,
            0,
            "uniform バインドは 16B 倍数が規約"
        );
        let wgsl = SHADER_VERTEX_PULL;
        for needle in [
            "view_proj: mat4x4<f32>,",
            "chunk_origin: vec4<f32>,",
            "@group(0) @binding(0) var<uniform> frame: FrameUniforms;",
        ] {
            assert!(
                wgsl.contains(needle),
                "WGSL FrameUniforms 表記乖離: {needle}"
            );
        }
    }

    /// wave 58 BH-4: vertex pull 実カーネルの entry/binding/絶対 index 語彙ピン。
    /// `draw(0..n)` の vertex_index がそのまま quads 配列の絶対 index になる
    /// (VBO 無し pull の前提語彙)。
    #[test]
    fn wgsl_vertex_pull_kernel_vocabulary() {
        let wgsl = SHADER_VERTEX_PULL;
        for needle in [
            "fn vs_pull(@builtin(vertex_index) vid: u32) -> VsOut {",
            "fn fs_pull(in: VsOut) -> @location(0) vec4<f32> {",
            "@group(0) @binding(1) var<storage, read> quads: array<PullQuad>;",
        ] {
            assert!(
                wgsl.contains(needle),
                "WGSL vertex pull 語彙ピン乖離: {needle}"
            );
        }
    }

    /// wave 58 BH-4: meshlet カリング WGSL は compute エミュレーションの
    /// 実カーネル (frame_worldgen が実 dispatch)。死にシェーダでないことを
    /// 表記ピン。
    #[test]
    fn meshlet_shader_is_real_compute_kernel() {
        let wgsl = SHADER_MESH_SHADER;
        assert!(
            wgsl.contains("@compute @workgroup_size(64, 1, 1)"),
            "compute dispatch エントリ"
        );
        assert!(
            wgsl.contains("// CPU ミラー (完全一致): src/frame_worldgen.rs の meshlet_cull_cpu"),
            "CPU ミラー注記 (frame_worldgen::meshlet_cull_cpu)"
        );
    }
}
