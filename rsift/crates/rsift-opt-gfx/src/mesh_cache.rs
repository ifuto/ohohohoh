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
        let data = fs::read(&path).ok()?;
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

    pub fn put(&mut self, mesh: &BuiltChunkMesh, section_rle: &[RleSection], section_y: i32) -> bool {
        if !self.enabled || mesh.is_empty {
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
    let read_i32 = |b: &[u8], o: &mut usize| -> Result<i32, String> {
        Ok(read_u32(b, o)? as i32)
    };

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
        is_empty: vertices.is_empty(),
        vertices,
        indices,
    })
}

fn decode_mesh_v1(raw: &[u8], mut off: usize, expect_x: i32, expect_z: i32) -> Result<BuiltChunkMesh, String> {
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
        is_empty: vertices.is_empty(),
        vertices,
        indices,
    })
}
