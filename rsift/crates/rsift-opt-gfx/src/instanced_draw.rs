
//! Instanced Rendering for Non-cube Blocks - 草花作物は同一メッシュをDrawInstanced
//! TransformはStructuredBufferでGPUへ

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct InstanceData {
    pub world_pos: [f32; 3],
    pub _pad0: f32,
    pub tex_index: u32,
    pub ao: u32,
    pub _pad1: [u32; 2],
}

pub struct InstancedGroup {
    pub mesh_id: u32,
    pub instances: Vec<InstanceData>,
}

pub struct InstancedCollector {
    groups: std::collections::HashMap<u32, InstancedGroup>,
}

impl InstancedCollector {
    pub fn new() -> Self { Self { groups: std::collections::HashMap::new() } }

    pub fn add(&mut self, mesh_id: u32, pos: [f32;3], tex: u32, ao: u32) {
        let g = self.groups.entry(mesh_id).or_insert_with(|| InstancedGroup { mesh_id, instances: Vec::new() });
        g.instances.push(InstanceData { world_pos: pos, _pad0: 0.0, tex_index: tex, ao, _pad1: [0;2] });
    }

    pub fn groups(&self) -> impl Iterator<Item=&InstancedGroup> { self.groups.values() }

    pub fn total_instances(&self) -> usize { self.groups.values().map(|g| g.instances.len()).sum() }

    pub fn as_bytes(&self, mesh_id: u32) -> Option<&[u8]> {
        self.groups.get(&mesh_id).map(|g| bytemuck::cast_slice(&g.instances))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_data_layout_is_32_bytes() {
        assert_eq!(std::mem::size_of::<InstanceData>(), 32);
    }

    #[test]
    fn collector_groups_and_counts() {
        let mut c = InstancedCollector::new();
        assert_eq!(c.total_instances(), 0);
        assert!(c.as_bytes(7).is_none());
        c.add(1, [0.0, 64.0, 0.0], 10, 3);
        c.add(1, [4.0, 64.0, 0.0], 10, 2);
        c.add(2, [8.0, 70.0, 8.0], 20, 1);
        assert_eq!(c.total_instances(), 3);
        assert_eq!(c.groups().count(), 2);
        let g1 = c.groups().find(|g| g.mesh_id == 1).unwrap();
        assert_eq!(g1.instances.len(), 2);
        assert_eq!(g1.instances[1].world_pos[0], 4.0); // 追加順保持
        assert_eq!(g1.instances[1].ao, 2);
    }

    #[test]
    fn as_bytes_roundtrips_pod_instances() {
        let mut c = InstancedCollector::new();
        c.add(3, [1.0, 2.0, 3.0], 42, 5);
        let bytes = c.as_bytes(3).unwrap();
        assert_eq!(bytes.len(), 32);
        let back: &[InstanceData] = bytemuck::cast_slice(bytes);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].tex_index, 42);
        assert_eq!(back[0].ao, 5);
        assert_eq!(back[0].world_pos, [1.0, 2.0, 3.0]);
    }
    /// 【wave 184 GD】削除済み API `clear()` の再出現 lexeme pin (adversarial
    /// 174-c・23 例目 非検出回収、wave 26 例未採から 1 回収): 削除 truth 証跡として
    /// 宣言形が再起しないことを機械 pin。自己言及 vacuous 回避のため検出
    /// 語彙は分割記述 (doc 証跡条文には `fn ` 接頭で択定範囲外)。
    #[test]
    fn gd_removed_clear_lexeme() {
        let src = include_str!("instanced_draw.rs");
        let lex = concat!("fn cle", "ar");
        assert!(
            !src.contains(lex),
            "削除 API `clear()` の宣言再来を検出 → 死救出は lint/テスト限界で"
        );
    }

}
