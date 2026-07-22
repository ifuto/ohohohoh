
//! bytemuck + zerocopyゼロコピー基盤 - GPU転送コピー0回

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuUploadHeader {
    pub magic: u32,
    pub version: u32,
    pub vertex_count: u32,
    pub index_count: u32,
}

pub fn cast_slice_to_bytes<T: Pod>(slice: &[T]) -> &[u8] {
    bytemuck::cast_slice(slice)
}

pub fn cast_bytes_to_slice<T: Pod>(bytes: &[u8]) -> Option<&[T]> {
    if bytes.len() % std::mem::size_of::<T>() != 0 {
        return None;
    }
    // アライメント検査: bytemuck::cast_slice は入力先頭番地が T の align を
    // 満たさない場合に**パニック**する (TargetAlignmentGreaterAndInputNotAligned)。
    // 特に空の `Vec<u8>` は dangling ポインタ (align 1) を返すため、
    // 「長さ 0 は常に合法」と信じた呼出側で致命的パニックとなる
    // (監査 2026-07-22 M-4: render_pipeline::frame() の quad_budget 経路で
    //  CI 診断 B1-B22 により実観測された latent panic)。Option 拒否に統一する。
    if (bytes.as_ptr() as usize) % std::mem::align_of::<T>() != 0 {
        return None;
    }
    Some(bytemuck::cast_slice(bytes))
}

pub fn zero_copy_vertex_upload(vertices: &[crate::chunk_mesh::Quantized12ByteVertex]) -> (&GpuUploadHeader, &[u8]) {
    // ヘッダーを先頭に付けてゼロコピーでGPUへ
    // 実際はUpload Heapに直接書き込む想定
    static HEADER: GpuUploadHeader = GpuUploadHeader { magic: 0x52534946, version: 1, vertex_count: 0, index_count: 0 };
    ( &HEADER, cast_slice_to_bytes(vertices) )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_mesh::Quantized12ByteVertex;

    #[test]
    fn cast_roundtrip_preserves_bytes() {
        let verts: Vec<Quantized12ByteVertex> = (0..3)
            .map(|i| Quantized12ByteVertex::encode(i as f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0))
            .collect();
        let bytes = cast_slice_to_bytes(&verts);
        assert_eq!(bytes.len(), 3 * 12);
        let back: &[Quantized12ByteVertex] = cast_bytes_to_slice(bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(cast_slice_to_bytes(back), bytes);
    }

    #[test]
    fn cast_bytes_to_slice_rejects_ragged_buffer() {
        let raw = [0u8; 13]; // 頂点 12B の倍数でない → None
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&raw).is_none());
        // 空バッファは Some(空) (0 % 12 == 0)。bytemuck は bytemuck::cast_slice
        // で目的型 align (4B) のポインタ検査を走らせるため、スタックの
        // [u8; 0] (align 1、番地は実行依存) ではなく align(4) 保証の受け皿で
        // 固定する (テストをポインタ配置の偶然に寄せない)。
        #[repr(align(4))]
        struct Aligned4([u8; 0]);
        let empty = Aligned4([]);
        let ok: &[Quantized12ByteVertex] = cast_bytes_to_slice(&empty.0).unwrap();
        assert!(ok.is_empty());
    }

    #[test]
    fn upload_header_wire_format() {
        let verts = [Quantized12ByteVertex::encode(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0)];
        let (header, payload) = zero_copy_vertex_upload(&verts);
        assert_eq!(header.magic, 0x5253_4946); // "RSIF" LE
        assert_eq!(header.version, 1);
        assert_eq!(payload.len(), 12);
        assert_eq!(std::mem::size_of::<GpuUploadHeader>(), 16); // repr(C) 4x u32
    }

    #[test]
    fn cast_bytes_to_slice_empty_vec_is_none_not_panic() {
        // M-4 実回帰: 空の Vec<u8> (dangling ptr, align 1) を渡すと旧実装は
        // bytemuck::cast_slice のアライメント検査でパニックした。
        // render_pipeline::frame() の quad_budget 経路は pull バイト 0 の
        // フレームでこれに到達し得る (デモ/空地形の通常実行でも)。
        let empty: Vec<u8> = Vec::new();
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&empty).is_none());
        assert!(cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(&empty).is_none());
        // ラギッドは引き続き None、実バッファは従来どおり Some。
        let ragged = vec![0u8; 10];
        assert!(cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(&ragged).is_none());
        let real = vec![0u8; 24];
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&real).is_some());
    }
}
