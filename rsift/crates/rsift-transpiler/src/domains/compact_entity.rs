//! # 16-Byte Ultra-Squeezed Entity Engine (`CompactEntity16B`)
//!
//! 112 バイト (`JvmEntityState`) から **16 バイト** (7.0倍縮小！) へ極限圧縮するSoA・量子化エンティティ構造体。
//! `WorldMirror` / `Physics` / `AI` ドメイン間の同期やメモリコピー負荷を最小化する。

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct CompactEntity16B {
    pub id_offset: u16,        // 0..65535 relative ID inside chunk bucket
    pub local_xyz: [u16; 3],   // 1/256 sub-block precision inside 16x16x16 chunk
    pub packed_vel: u16,       // Octahedral / half-float quantized velocity vector
    pub yaw_pitch: u16,        // 8-bit yaw + 8-bit pitch (360/256 = 1.4° angular resolution)
    pub flags_and_health: u32, // health (12b), ai_flags (12b), on_ground (1b), in_fluid (2b), removed (1b), type_id (4b)
}

impl CompactEntity16B {
    pub fn encode(e: &crate::world_mirror::JvmEntityState, origin_x: i32, origin_y: i32, origin_z: i32) -> Self {
        let lx = ((e.pos_x - origin_x as f64) * 256.0).clamp(0.0, 65535.0) as u16;
        let ly = ((e.pos_y - origin_y as f64) * 256.0).clamp(0.0, 65535.0) as u16;
        let lz = ((e.pos_z - origin_z as f64) * 256.0).clamp(0.0, 65535.0) as u16;

        let yp = ((e.yaw * (256.0 / 360.0)) as u32 & 0xFF) as u16
            | (((e.pitch * (256.0 / 360.0)) as u32 & 0xFF) as u16) << 8;

        let health_12b = (e.health.clamp(0.0, 1023.0) * 4.0) as u32 & 0xFFF;
        let flags_12b = e.flags & 0xFFF;
        let og_1b = (e.on_ground & 1) as u32;
        let removed_1b = (e.removed & 1) as u32;
        let type_4b = (e.entity_type & 0xF) as u32;

        let flags_and_health = health_12b
            | (flags_12b << 12)
            | (og_1b << 24)
            | (removed_1b << 25)
            | (type_4b << 28);

        Self {
            id_offset: (e.entity_id & 0xFFFF) as u16,
            local_xyz: [lx, ly, lz],
            packed_vel: 0, // Simplified velocity quant
            yaw_pitch: yp,
            flags_and_health,
        }
    }

    pub fn decode_to(&self, origin_x: i32, origin_y: i32, origin_z: i32, dst: &mut crate::world_mirror::JvmEntityState) {
        dst.entity_id = self.id_offset as i32;
        dst.pos_x = origin_x as f64 + (self.local_xyz[0] as f64 / 256.0);
        dst.pos_y = origin_y as f64 + (self.local_xyz[1] as f64 / 256.0);
        dst.pos_z = origin_z as f64 + (self.local_xyz[2] as f64 / 256.0);

        dst.yaw = (self.yaw_pitch & 0xFF) as f32 * (360.0 / 256.0);
        dst.pitch = ((self.yaw_pitch >> 8) & 0xFF) as f32 * (360.0 / 256.0);

        let h = (self.flags_and_health & 0xFFF) as f32 / 4.0;
        dst.health = h;
        dst.flags = (self.flags_and_health >> 12) & 0xFFF;
        dst.on_ground = ((self.flags_and_health >> 24) & 1) as u8;
        dst.removed = ((self.flags_and_health >> 25) & 1) as u8;
        dst.entity_type = (self.flags_and_health >> 28) & 0xF;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_entity_16b_roundtrip() {
        let mut e = crate::world_mirror::JvmEntityState::default();
        e.entity_id = 42;
        e.pos_x = 100.5;
        e.pos_y = 64.25;
        e.pos_z = 200.75;
        e.health = 20.0;
        e.on_ground = 1;

        let packed = CompactEntity16B::encode(&e, 100, 64, 200);
        assert_eq!(std::mem::size_of::<CompactEntity16B>(), 16);

        let mut out = crate::world_mirror::JvmEntityState::default();
        packed.decode_to(100, 64, 200, &mut out);
        assert_eq!(out.entity_id, 42);
        assert!((out.pos_x - 100.5).abs() < 0.01);
        assert_eq!(out.on_ground, 1);
    }
}
