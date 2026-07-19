//! wgpu-backed OpenGL 3.3 feature equivalents (Tier 1).
//! Maps VAO/UBO/Instancing/MultiDraw/TimerQuery → wgpu without requiring GL context.

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
    pub fn from_pod<T: bytemuck::Pod>(items: &[T]) -> Self {
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
    samples: Vec<f32>,
}

impl TimerQueryCompat {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            start: None,
            last_ms: 0.0,
            samples: Vec::with_capacity(64),
        }
    }

    pub fn begin(&mut self) {
        self.start = Some(Instant::now());
    }

    pub fn end(&mut self) -> f32 {
        let ms = self
            .start
            .take()
            .map(|t| t.elapsed().as_secs_f32() * 1000.0)
            .unwrap_or(0.0);
        self.last_ms = ms;
        self.samples.push(ms);
        if self.samples.len() > 120 {
            self.samples.remove(0);
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
}
