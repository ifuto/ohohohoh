//! wgpu-backed OpenGL 3.3 feature equivalents (Tier 1).
//! Maps VAO/UBO/Instancing/MultiDraw/TimerQuery → wgpu without requiring GL context.

use std::collections::VecDeque;
use std::time::Instant;

/// DrawIndexedIndirectArgs — GL `glMultiDrawElementsIndirect` / wgpu equivalent.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrawIndexedIndirectArgs {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

/// VAO-compat: records vertex layout + buffer offsets (bound at draw time on wgpu).
#[derive(Debug, Clone)]
pub struct GlVaoCompat {
    pub stride: u32,
    pub attributes: Vec<VaoAttrib>,
    pub index_format_u16: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VaoAttrib {
    pub location: u32,
    pub offset: u32,
    pub format_floats: u8, // 1..=4
}

impl GlVaoCompat {
    pub fn terrain_compact() -> Self {
        Self {
            stride: 12,
            attributes: vec![
                VaoAttrib {
                    location: 0,
                    offset: 0,
                    format_floats: 1, // packed pos
                },
                VaoAttrib {
                    location: 1,
                    offset: 4,
                    format_floats: 1,
                },
                VaoAttrib {
                    location: 2,
                    offset: 8,
                    format_floats: 1,
                },
            ],
            index_format_u16: true,
        }
    }
}

/// UBO — CPU mirror + dirty flag; upload via queue.write_buffer when device present.
#[derive(Debug, Clone)]
pub struct UniformBufferObject<T: Copy + bytemuck::Pod> {
    pub data: T,
    pub dirty: bool,
    pub binding: u32,
}

impl<T: Copy + bytemuck::Pod + Default> UniformBufferObject<T> {
    pub fn new(binding: u32) -> Self {
        Self {
            data: T::default(),
            dirty: true,
            binding,
        }
    }

    pub fn set(&mut self, value: T) {
        self.data = value;
        self.dirty = true;
    }

    pub fn bytes(&self) -> &[u8] {
        bytemuck::bytes_of(&self.data)
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUbo {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    pub time_fog: [f32; 4],
}

/// Instance attribute buffer (flora / entities).
#[derive(Debug, Default)]
pub struct InstanceBuffer {
    pub raw: Vec<u8>,
    pub stride: u32,
    pub count: u32,
}

impl InstanceBuffer {
    /// `T` は ZST 不可 (stride 0 の instance buffer は GPU 側で受理されず
    /// 意味を成さない — 2026-07-22 wave 38 で fail-loud 契約化)。
    pub fn from_pod<T: bytemuck::Pod>(items: &[T]) -> Self {
        assert!(
            std::mem::size_of::<T>() > 0,
            "InstanceBuffer::from_pod 契約違反: ZST は stride 0 で不可"
        );
        Self {
            raw: bytemuck::cast_slice(items).to_vec(),
            stride: std::mem::size_of::<T>() as u32,
            count: items.len() as u32,
        }
    }
}

#[derive(Debug, Default)]
pub struct MultiDrawIndirectCompat {
    pub commands: Vec<DrawIndexedIndirectArgs>,
}

impl MultiDrawIndirectCompat {
    pub fn push(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    ) {
        self.commands.push(DrawIndexedIndirectArgs {
            index_count,
            instance_count,
            first_index,
            base_vertex,
            first_instance,
        });
    }

    pub fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.commands)
    }

    pub fn draw_count(&self) -> u32 {
        self.commands.len() as u32
    }

    /// Total triangles if each index_count is for triangle lists.
    /// index_count が 3 の倍数でない場合の端数は切り捨てる
    /// (primitive 未完成分は描画されない規則と同じ解釈)。
    pub fn total_triangles(&self) -> u64 {
        self.commands
            .iter()
            .map(|c| (c.index_count as u64 / 3) * c.instance_count as u64)
            .sum()
    }
}

/// Timer query — prefers CPU Instant; optional wgpu timestamp when available.
#[derive(Debug)]
pub struct TimerQueryCompat {
    label: String,
    start: Option<Instant>,
    last_ms: f32,
    samples: VecDeque<f32>,
}

impl TimerQueryCompat {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            start: None,
            last_ms: 0.0,
            samples: VecDeque::with_capacity(64),
        }
    }

    pub fn begin(&mut self) {
        self.start = Some(Instant::now());
    }

    /// **契約 (2026-07-22 wave 38 根治)**: `begin` 未対応の `end` (二重
    /// end 含む) は **sample を記録せず** 前値 `last_ms` を返す。
    /// 旧実装は 0.0 を samples に混入して average を汚染していた
    /// (観測欠測の混入 — drs wave 23 の NaN と同型)。
    /// 上限 120 件の保持は VecDeque の O(1) front-drop
    /// (旧 `Vec::remove(0)` の O(n) memmove を根治)。
    pub fn end(&mut self) -> f32 {
        let Some(t) = self.start.take() else {
            return self.last_ms;
        };
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        self.last_ms = ms;
        self.samples.push_back(ms);
        if self.samples.len() > 120 {
            self.samples.pop_front();
        }
        ms
    }

    pub fn average_ms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f32>() / self.samples.len() as f32
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn last_ms(&self) -> f32 {
        self.last_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdi_and_ubo() {
        let mut mdi = MultiDrawIndirectCompat::default();
        mdi.push(36, 1, 0, 0, 0);
        mdi.push(36, 4, 36, 0, 0);
        assert_eq!(mdi.draw_count(), 2);
        assert_eq!(mdi.total_triangles(), 12 + 48);
        let mut ubo = UniformBufferObject::<FrameUbo>::new(0);
        ubo.set(FrameUbo::default());
        assert!(ubo.dirty);
        assert_eq!(ubo.bytes().len(), std::mem::size_of::<FrameUbo>());
    }

    /// wave 38-1: timer は begin 未対応の end を記録しない (混入=旧赤)。
    #[test]
    fn timer_beginless_end_does_not_pollute_samples() {
        let mut t = TimerQueryCompat::new("probe");
        assert_eq!(t.end(), 0.0, "begin 前: 前値 (初期 0) を返す");
        assert_eq!(t.end(), 0.0);
        t.begin();
        let ms = t.end();
        assert!(ms.is_finite() && ms >= 0.0);
        assert_eq!(t.end(), ms, "二重 end は前値を返すのみ");
        t.begin();
        let _ = t.end();
        // samples は実測 2 回分のみ (旧実装は先頭 2 件の 0.0 偽装を混入)
        assert_eq!(t.samples.len(), 2);
        assert_eq!(t.average_ms(), (t.samples[0] + t.samples[1]) / 2.0);
        assert_eq!(t.label(), "probe");
        assert_eq!(t.last_ms(), *t.samples.back().unwrap());
    }

    /// wave 38-2: 120 件上限 (VecDeque O(1) front-drop) — 構造のみピン
    /// (実値は実時間由来で非決定のため狙わない)。
    #[test]
    fn timer_window_capped_at_120() {
        let mut t = TimerQueryCompat::new("w");
        for _ in 0..130 {
            t.begin();
            let _ = t.end();
        }
        assert_eq!(t.samples.len(), 120);
        assert!(t.average_ms().is_finite());
    }

    /// wave 38-3: InstanceBuffer の wire 厳密 + ZST 拒否。
    #[test]
    fn instance_buffer_exact_wire_and_zst_rejected() {
        let ib = InstanceBuffer::from_pod(&[1u8, 2, 3]);
        assert_eq!(ib.stride, 1);
        assert_eq!(ib.count, 3);
        assert_eq!(ib.raw, vec![1u8, 2, 3]);
        let ib32 = InstanceBuffer::from_pod(&[7u32, 8]);
        assert_eq!(ib32.stride, 4);
        assert_eq!(ib32.count, 2);
        assert_eq!(
            ib32.raw,
            bytemuck::cast_slice::<u32, u8>(&[7u32, 8]).to_vec()
        );
    }

    #[test]
    #[should_panic(expected = "InstanceBuffer::from_pod 契約違反")]
    fn instance_buffer_rejects_zst() {
        let _ = InstanceBuffer::from_pod(&[(), ()]);
    }

    /// wave 38-4: total_triangles 端数切捨て + UBO 遷移 + FrameUbo wire ピン。
    #[test]
    fn triangles_fraction_floor_and_ubo_transitions() {
        let mut mdi = MultiDrawIndirectCompat::default();
        mdi.push(35, 2, 0, 0, 0); // 35/3=11 (端数切捨て) ×2 インスタンス
        assert_eq!(mdi.total_triangles(), 22);
        assert_eq!(mdi.bytes().len(), 20);
        let mut ubo = UniformBufferObject::<FrameUbo>::new(3);
        assert!(ubo.dirty, "初期は dirty (アップロード要)");
        ubo.mark_clean();
        assert!(!ubo.dirty);
        ubo.set(FrameUbo::default());
        assert!(ubo.dirty, "set で再 dirty");
        assert_eq!(ubo.binding, 3);
        assert_eq!(std::mem::size_of::<FrameUbo>(), 96); // 64+16+16
    }
}
