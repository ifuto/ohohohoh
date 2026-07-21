
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
    if bytes.len() % std::mem::size_of::<T>() != 0 { return None; }
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
}
