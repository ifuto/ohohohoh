//! Disk mesh cache — RLE palette header + zstd compressed mesh (skip rebuild on revisit).

use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex};
use crate::section_rle::RleSection;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, trace, warn};

const CACHE_VERSION: u32 = 2;
const CACHE_DIR: &str = "rsift-mesh-cache";

#[derive(Debug)]
pub struct MeshDiskCache {
    root: PathBuf,
    hits: u64,
    misses: u64,
    enabled: bool,
}

impl MeshDiskCache {
    pub fn new(game_dir: &Path, enabled: bool) -> Self {
        let root = game_dir.join(CACHE_DIR);
        if enabled {
            let _ = fs::create_dir_all(&root);
        }
        Self {
            root,
            hits: 0,
            misses: 0,
            enabled,
        }
    }

    pub fn adaptive(game_dir: &Path) -> Self {
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        Self::new(game_dir, rp.mesh_disk_cache)
    }

    fn key_path(&self, cx: i32, cz: i32, section_y: i32) -> PathBuf {
        self.root.join(format!("{}_{}_{}.rmesh", cx, cz, section_y))
    }

    pub fn get(&mut self, cx: i32, cz: i32, section_y: i32) -> Option<BuiltChunkMesh> {
        if !self.enabled {
            return None;
        }
        let path = self.key_path(cx, cz, section_y);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                // 2026-07-21 テスト駆動修正: キー不在は従来 misses に計上されず
                // (0,0) のままだった。キャッシュ統計の標準語彙では不在参照も
                // miss なので計上する (decode 失敗時の計上と対称化)。
                self.misses += 1;
                return None;
            }
        };
        match decode_mesh(&data, cx, cz) {
            Ok(m) => {
                self.hits += 1;
                trace!("[MeshCache] hit ({}, {})", cx, cz);
                Some(m)
            }
            Err(e) => {
                warn!("[MeshCache] corrupt entry {:?}: {}", path, e);
                let _ = fs::remove_file(path);
                self.misses += 1;
                None
            }
        }
    }

    pub fn put(
        &mut self,
        mesh: &BuiltChunkMesh,
        section_rle: &[RleSection],
        section_y: i32,
    ) -> bool {
        if !self.enabled || mesh.is_empty() {
            return false;
        }
        let path = self.key_path(mesh.chunk_x, mesh.chunk_z, section_y);
        match encode_mesh(mesh, section_rle) {
            Ok(bytes) => {
                if let Ok(mut f) = fs::File::create(&path) {
                    let _ = f.write_all(&bytes);
                    debug!(
                        "[MeshCache] stored ({}, {}) {} bytes (RLE header)",
                        mesh.chunk_x,
                        mesh.chunk_z,
                        bytes.len()
                    );
                    return true;
                }
            }
            Err(e) => warn!("[MeshCache] encode failed: {}", e),
        }
        false
    }

    /// Drop cached meshes for a chunk column so live world edits remesh.
    pub fn invalidate_chunk(&mut self, cx: i32, cz: i32) {
        if !self.enabled {
            return;
        }
        if let Ok(entries) = fs::read_dir(&self.root) {
            let prefix = format!("{}_{}_", cx, cz);
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&prefix) && name.ends_with(".rmesh") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

fn encode_mesh(mesh: &BuiltChunkMesh, section_rle: &[RleSection]) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    raw.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    raw.extend_from_slice(&mesh.chunk_x.to_le_bytes());
    raw.extend_from_slice(&mesh.chunk_z.to_le_bytes());
    raw.extend_from_slice(&(section_rle.len() as u16).to_le_bytes());
    for sec in section_rle {
        let bytes = sec.to_bytes();
        raw.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        raw.extend_from_slice(&bytes);
    }
    raw.extend_from_slice(&(mesh.vertices.len() as u32).to_le_bytes());
    raw.extend_from_slice(&(mesh.indices.len() as u32).to_le_bytes());
    raw.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    raw.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    zstd::encode_all(raw.as_slice(), 3).map_err(|e| e.to_string())
}

fn decode_mesh(data: &[u8], expect_x: i32, expect_z: i32) -> Result<BuiltChunkMesh, String> {
    let raw = zstd::decode_all(data).map_err(|e| e.to_string())?;
    let mut off = 0usize;
    let read_u32 = |b: &[u8], o: &mut usize| -> Result<u32, String> {
        if *o + 4 > b.len() {
            return Err("truncated".into());
        }
        let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
        *o += 4;
        Ok(v)
    };
    let read_u16 = |b: &[u8], o: &mut usize| -> Result<u16, String> {
        if *o + 2 > b.len() {
            return Err("truncated".into());
        }
        let v = u16::from_le_bytes(b[*o..*o + 2].try_into().unwrap());
        *o += 2;
        Ok(v)
    };
    let read_i32 = |b: &[u8], o: &mut usize| -> Result<i32, String> { Ok(read_u32(b, o)? as i32) };

    let version = read_u32(&raw, &mut off)?;
    if version == 1 {
        return decode_mesh_v1(&raw, off, expect_x, expect_z);
    }
    if version != CACHE_VERSION {
        return Err("version mismatch".into());
    }
    let cx = read_i32(&raw, &mut off)?;
    let cz = read_i32(&raw, &mut off)?;
    if cx != expect_x || cz != expect_z {
        return Err("coord mismatch".into());
    }
    let sec_count = read_u16(&raw, &mut off)? as usize;
    for _ in 0..sec_count {
        let len = read_u32(&raw, &mut off)? as usize;
        if off + len > raw.len() {
            return Err("truncated rle header".into());
        }
        // wave 61 BK: rle ペイロードを実検証。旧来は長さだけ見てスキップする
        // write-only 領域だった — 破損 rle (Σcount≠4096、wire 厳格違反) を
        // 含むキャッシュエントリはここで拒否し再構築へ倒す (section_rle の
        // from_bytes 厳格化で初めて検証可能になった)。
        if RleSection::from_bytes(&raw[off..off + len]).is_none() {
            return Err("corrupt rle section".into());
        }
        off += len;
    }
    let vlen = read_u32(&raw, &mut off)? as usize;
    let ilen = read_u32(&raw, &mut off)? as usize;
    let vbytes = vlen * std::mem::size_of::<Quantized12ByteVertex>();
    let ibytes = ilen * 4;
    if off + vbytes + ibytes > raw.len() {
        return Err("truncated mesh".into());
    }
    let vertices: Vec<Quantized12ByteVertex> =
        bytemuck::cast_slice(&raw[off..off + vbytes]).to_vec();
    off += vbytes;
    let indices: Vec<u32> = bytemuck::cast_slice(&raw[off..off + ibytes]).to_vec();
    Ok(BuiltChunkMesh {
        chunk_x: cx,
        chunk_z: cz,
        vertices,
        indices,
    })
}

fn decode_mesh_v1(
    raw: &[u8],
    mut off: usize,
    expect_x: i32,
    expect_z: i32,
) -> Result<BuiltChunkMesh, String> {
    let read_u32 = |b: &[u8], o: &mut usize| -> Result<u32, String> {
        if *o + 4 > b.len() {
            return Err("truncated".into());
        }
        let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
        *o += 4;
        Ok(v)
    };
    let cx = read_u32(raw, &mut off)? as i32;
    let cz = read_u32(raw, &mut off)? as i32;
    if cx != expect_x || cz != expect_z {
        return Err("coord mismatch".into());
    }
    let vlen = read_u32(raw, &mut off)? as usize;
    let ilen = read_u32(raw, &mut off)? as usize;
    let vbytes = vlen * std::mem::size_of::<Quantized12ByteVertex>();
    let ibytes = ilen * 4;
    if off + vbytes + ibytes > raw.len() {
        return Err("truncated mesh".into());
    }
    let vertices: Vec<Quantized12ByteVertex> =
        bytemuck::cast_slice(&raw[off..off + vbytes]).to_vec();
    off += vbytes;
    let indices: Vec<u32> = bytemuck::cast_slice(&raw[off..off + ibytes]).to_vec();
    Ok(BuiltChunkMesh {
        chunk_x: cx,
        chunk_z: cz,
        vertices,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::SectionPalette;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp_root(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rsift_mesh_cache_test_{}_{}_{}",
            std::process::id(),
            tag,
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample_mesh(cx: i32, cz: i32) -> BuiltChunkMesh {
        let v = Quantized12ByteVertex::encode(1.0, 2.0, 3.0, 0.0, 1.0, 0.0, 0.5, 0.25);
        BuiltChunkMesh {
            chunk_x: cx,
            chunk_z: cz,
            vertices: vec![v, v, v, v],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    fn one_rle() -> Vec<RleSection> {
        let mut p: SectionPalette = [0u16; 4096];
        p[0] = 7;
        vec![RleSection::encode(&p)]
    }

    fn verts_bytes(m: &BuiltChunkMesh) -> Vec<u8> {
        bytemuck::cast_slice::<Quantized12ByteVertex, u8>(&m.vertices).to_vec()
    }

    #[test]
    fn disabled_cache_is_inert_and_creates_nothing() {
        let root = tmp_root("disabled");
        let mut c = MeshDiskCache::new(&root, false);
        assert!(!root.join(CACHE_DIR).exists(), "disabled must not mkdir");
        assert!(!c.put(&sample_mesh(1, 2), &one_rle(), 0));
        assert!(c.get(1, 2, 0).is_none());
        c.invalidate_chunk(1, 2); // no-op、パニックしないこと
        assert_eq!(c.stats(), (0, 0));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn put_rejects_empty_mesh() {
        let root = tmp_root("empty");
        let mut c = MeshDiskCache::new(&root, true);
        // wave 59 BI-A: 空判定は vertices からの導出 — 空メッシュを直接構築する
        // (旧実装は is_empty=true 手書きで「頂点非空だが空」非整合を発生させていた)。
        let m = BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            vertices: vec![],
            indices: vec![],
        };
        assert!(!c.put(&m, &one_rle(), 0));
        assert!(c.get(0, 0, 0).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    /// wave 61 BK: rle ペイロード破損 (Σcount≠4096) のエントリは拒否される
    /// (旧来は長さだけ見てスキップする write-only 領域だった)。
    #[test]
    fn decode_rejects_corrupt_rle_payload() {
        let m = sample_mesh(1, 2);
        let packed = encode_mesh(&m, &one_rle()).expect("encode ok");
        let mut raw = zstd::decode_all(packed.as_slice()).expect("zstd roundtrip");
        // decode_mesh 側 wire 配置: [u32 version][i32 cx][i32 cz][u16 slen]
        //   [u32 len][rle blob] → blob は offset 18 から。
        // blob 内配置: [run_count:u16][block:u16 count:u16]* → run0 count は
        //   offset 18+4=22 の LE 2B。4096→4095 ではなく run0(=1) を 4095 に
        //   改竄して Σ=8190≠4096 を作る。
        raw[22] = 0xFF;
        raw[23] = 0x0F;
        let corrupt = zstd::encode_all(raw.as_slice(), 3).expect("re-encode");
        assert!(
            decode_mesh(&corrupt, 1, 2).is_err(),
            "Σcount≠4096 の rle を含むエントリは拒否 (再構築へ倒す)"
        );
    }

    #[test]
    fn put_get_roundtrip_bits_and_stats() {
        let root = tmp_root("roundtrip");
        let mut c = MeshDiskCache::new(&root, true);
        let src = sample_mesh(-7, 13);
        let expect_v = verts_bytes(&src);
        assert!(c.put(&src, &one_rle(), 3));
        let got = c.get(-7, 13, 3).expect("hit after put");
        assert_eq!((got.chunk_x, got.chunk_z), (-7, 13));
        assert!(!got.is_empty());
        assert_eq!(verts_bytes(&got), expect_v, "vertex bytes must round-trip");
        assert_eq!(got.indices, src.indices);
        assert_eq!(c.stats(), (1, 0));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_key_counts_miss() {
        let root = tmp_root("miss");
        let mut c = MeshDiskCache::new(&root, true);
        assert!(c.get(9, 9, 0).is_none());
        assert_eq!(c.stats(), (0, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_entry_is_removed_and_counted_as_miss() {
        let root = tmp_root("corrupt");
        let mut c = MeshDiskCache::new(&root, true);
        let path = c.key_path(4, 5, 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"definitely-not-zstd").unwrap();
        assert!(c.get(4, 5, 0).is_none());
        assert!(!path.exists(), "corrupt entry must be deleted");
        assert_eq!(c.stats(), (0, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn coord_mismatch_blob_is_rejected_and_removed() {
        // (7,8) 用に encode した blob を (9,8) のキー名で置いた場合、
        // ヘッダ coords と要求 coords の不整合で拒否・削除されること。
        let root = tmp_root("mismatch");
        let mut c = MeshDiskCache::new(&root, true);
        assert!(c.put(&sample_mesh(7, 8), &one_rle(), 0));
        let good = c.key_path(7, 8, 0);
        let wrong = c.key_path(9, 8, 0);
        fs::rename(&good, &wrong).unwrap();
        assert!(
            c.get(9, 8, 0).is_none(),
            "foreign coords must not be served"
        );
        assert!(!wrong.exists(), "mismatched entry must be deleted");
        assert_eq!(c.stats(), (0, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalidate_chunk_prefix_scoping_is_exact() {
        // "1_2_" は "1_20_" や "2_1_" に一致してはいけない (部分文字列事故の固定)。
        let root = tmp_root("invalidate");
        let mut c = MeshDiskCache::new(&root, true);
        assert!(c.put(&sample_mesh(1, 2), &one_rle(), 0));
        assert!(c.put(&sample_mesh(1, 20), &one_rle(), 0));
        assert!(c.put(&sample_mesh(2, 1), &one_rle(), 0));
        c.invalidate_chunk(1, 2);
        assert!(!c.key_path(1, 2, 0).exists());
        assert!(c.key_path(1, 20, 0).exists());
        assert!(c.key_path(2, 1, 0).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn v1_format_still_decodes() {
        // CACHE_VERSION=1 の旧レイアウト (RLE ヘッダ無し) を手作りし、
        // 後方互換デコード経路 decode_mesh_v1 が生きていることを固定する。
        let root = tmp_root("v1");
        let mut c = MeshDiskCache::new(&root, true);
        let src = sample_mesh(2, -3);
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes()); // version 1
        raw.extend_from_slice(&2i32.to_le_bytes());
        raw.extend_from_slice(&(-3i32).to_le_bytes());
        raw.extend_from_slice(&(src.vertices.len() as u32).to_le_bytes());
        raw.extend_from_slice(&(src.indices.len() as u32).to_le_bytes());
        raw.extend_from_slice(bytemuck::cast_slice(&src.vertices));
        raw.extend_from_slice(bytemuck::cast_slice(&src.indices));
        let blob = zstd::encode_all(raw.as_slice(), 3).unwrap();
        let path = c.key_path(2, -3, 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &blob).unwrap();
        let got = c.get(2, -3, 0).expect("v1 entry must decode");
        assert_eq!(verts_bytes(&got), verts_bytes(&src));
        assert_eq!(got.indices, src.indices);
        let _ = fs::remove_dir_all(&root);
    }
}
