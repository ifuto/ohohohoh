//! Bobby 逆輸入 — サーバーチャンクのローカルキャッシュ。
//!
//! Bobby の実態: 受信したサーバーチャンクをローカルディスクに保存し、
//! 次回接続時に「自分のレンダ距離外」でも既知の地形を表示する。
//! 本モジュールはストレージ層 (圧縮/耐久性/LRU整合) を完全実装する。
//!
//! 形式 (`.rcc` = Rsift Cached Chunk):
//! ```text
//!   magic   u32  "RCC1"
//!   version u32
//!   dim     u8   (0=ow,1=nether,2=end)
//!   cx, cz  i32
//!   time_ms u64
//!   crc32   u32  (payload)
//!   zstd    payload (level 13 で圧縮)
//! ```
//!
//! 再起動時はディスク上のヘッダから time_ms を復元して index を再構築する
//! ため、LRU の「最古から捨てる」順序はプロセスを跨いでも保持される
//! (時刻同値は (dim, cx, cz) で tie-break し、削除順は完全に決定的)。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const RCC_MAGIC: u32 = 0x5243_4331; // "RCC1"
pub const RCC_VERSION: u32 = 1;
pub const HEADER_BYTES: usize = 4 + 4 + 1 + 3;
pub const META_TAIL: usize = 8 + 4;

#[derive(Debug, Clone)]
pub struct CachedChunk {
    pub dim: u8,
    pub cx: i32,
    pub cz: i32,
    pub time_ms: u64,
    pub payload: Vec<u8>, // 非圧縮のセクション bytes (受信形そのまま)
}

pub struct BobbyCache {
    root: PathBuf,
    server_key: u64, // ip+port ハッシュで消毒済みのキー
    max_bytes: u64,
    /// (dim, cx, cz) → (パス, 非圧縮サイズ, time_ms)
    index: Mutex<HashMap<(u8, i32, i32), IndexEntry>>,
}

#[derive(Clone, Copy)]
struct IndexEntry {
    file_size: u64,
    time_ms: u64,
}

impl BobbyCache {
    /// `server_addr` は "ip:port" 文字列。同名で衝突しないよう FNV ハッシュに。
    pub fn new(root: &Path, server_addr: &str, max_bytes: u64) -> std::io::Result<Self> {
        let server_key = fnv1a_64(server_addr.as_bytes());
        let dir = root
            .join("bobby_cache")
            .join(format!("{:016x}", server_key));
        std::fs::create_dir_all(&dir)?;
        let cache = Self {
            root: dir,
            server_key,
            max_bytes: max_bytes.max(1 << 20),
            index: Mutex::new(HashMap::new()),
        };
        cache.rebuild_index()?;
        Ok(cache)
    }

    fn rebuild_index(&self) -> std::io::Result<()> {
        let mut idx = self.index.lock().unwrap();
        idx.clear();
        for dim_dir in std::fs::read_dir(&self.root)?.flatten() {
            let dim_path = dim_dir.path();
            if !dim_path.is_dir() {
                continue;
            }
            // 次元ディレクトリ名は store が生成する "{dim}" (0..=2) のみ解釈する。
            // 解釈できない名 (外部混入のゴミ等) を dim=0 扱いすると、正当な
            // (0,cx,cz) エントリを索引上でキー衝突・上書きするため skip する。
            let Some(dim) = dim_path
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u8>().ok())
            else {
                continue;
            };
            for entry in std::fs::read_dir(&dim_path)?.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) != Some("rcc") {
                    continue;
                }
                if let Some((cx, cz)) = parse_chunk_name(&entry.path()) {
                    let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    // ディスクのヘッダに保存済みの time_ms を復元する。
                    // 0 埋めだと再起動後は全件「同時刻」になり、LRU の削除順が
                    // HashMap 反復順 (非決定) に堕ちる。読み取り・検証に失敗した
                    // 壊れファイルは 0 (最古) 扱いで真っ先に捨てる。
                    let time_ms = read_header_time_ms(&entry.path()).unwrap_or(0);
                    idx.insert((dim, cx, cz), IndexEntry { file_size, time_ms });
                }
            }
        }
        Ok(())
    }

    fn chunk_path(&self, dim: u8, cx: i32, cz: i32) -> PathBuf {
        self.root
            .join(format!("{dim}"))
            .join(format!("{cx}_{cz}.rcc"))
    }

    /// 保存 (圧縮 + 一時ファイル→rename の atomic 化)。
    pub fn store(&self, chunk: &CachedChunk) -> std::io::Result<()> {
        let path = self.chunk_path(chunk.dim, chunk.cx, chunk.cz);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let compressed = zstd_encode(&chunk.payload)?;
        let mut file_bytes = Vec::with_capacity(HEADER_BYTES + META_TAIL + compressed.len());
        file_bytes.extend_from_slice(&RCC_MAGIC.to_le_bytes());
        file_bytes.extend_from_slice(&RCC_VERSION.to_le_bytes());
        file_bytes.push(chunk.dim);
        file_bytes.extend_from_slice(&[0u8; 3]);
        file_bytes.extend_from_slice(&chunk.cx.to_le_bytes());
        file_bytes.extend_from_slice(&chunk.cz.to_le_bytes());
        file_bytes.extend_from_slice(&chunk.time_ms.to_le_bytes());
        file_bytes.extend_from_slice(&crc32(&chunk.payload).to_le_bytes());
        file_bytes.extend_from_slice(&compressed);

        let tmp = path.with_extension("rcc.tmp");
        std::fs::write(&tmp, &file_bytes)?;
        std::fs::rename(&tmp, &path)?;

        let meta_len = file_bytes.len() as u64;
        // poison 時に黙って index 更新を捨てるとディスクと索引が不整合になる。
        // 他のロック取得と同様、poison は panic として可視化する (誠実性)。
        self.index.lock().unwrap().insert(
            (chunk.dim, chunk.cx, chunk.cz),
            IndexEntry { file_size: meta_len, time_ms: chunk.time_ms },
        );
        self.enforce_lru()?;
        Ok(())
    }

    /// 読み出し。壊れてたら None (Bobby の「壊れキャッシュは捨てる」と同じ)。
    pub fn load(&self, dim: u8, cx: i32, cz: i32) -> std::io::Result<Option<CachedChunk>> {
        let path = self.chunk_path(dim, cx, cz);
        let Ok(mut bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        if bytes.len() < HEADER_BYTES + META_TAIL {
            return Ok(None);
        }
        if u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != RCC_MAGIC {
            return Ok(None);
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != RCC_VERSION {
            return Ok(None);
        }
        let dim_r = bytes[8];
        let cx_r = i32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let cz_r = i32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let time_ms = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
        let crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let payload_z: Vec<_> = bytes.drain(32..).collect();

        let payload = match zstd_decode(&payload_z) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        if crc32(&payload) != crc {
            return Ok(None);
        }
        if dim_r != dim || cx_r != cx || cz_r != cz {
            return Ok(None);
        }
        Ok(Some(CachedChunk { dim, cx, cz, time_ms, payload }))
    }

    /// 総使用サイズ。
    pub fn total_bytes(&self) -> u64 {
        self.index.lock().unwrap().values().map(|e| e.file_size).sum()
    }

    /// LRU: 古いものから max_bytes まで掃除 (Bobby の purge)。
    /// ソートキーは (time_ms, dim, cx, cz) — 時刻同値でも削除順が HashMap
    /// 反復順に依存せず完全に決定的になるよう tie-break する。
    pub fn enforce_lru(&self) -> std::io::Result<()> {
        let mut idx = self.index.lock().unwrap();
        let mut total: u64 = idx.values().map(|e| e.file_size).sum();
        if total <= self.max_bytes {
            return Ok(());
        }
        let mut entries: Vec<((u8, i32, i32), IndexEntry)> =
            idx.iter().map(|(k, v)| (*k, *v)).collect();
        entries.sort_by_key(|(k, e)| (e.time_ms, k.0, k.1, k.2));
        for (key, entry) in entries {
            if total <= self.max_bytes {
                break;
            }
            let path = self.chunk_path(key.0, key.1, key.2);
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(entry.file_size);
            idx.remove(&key);
        }
        Ok(())
    }

    pub fn server_key(&self) -> u64 {
        self.server_key
    }

    pub fn cached_count(&self) -> usize {
        self.index.lock().unwrap().len()
    }

    /// テスト専用: 索引上の time_ms を直接観測する
    /// (rebuild 後の時刻復元を、LRU 挙動経由ではなく直接検証するため)。
    #[cfg(test)]
    fn index_time_ms(&self, dim: u8, cx: i32, cz: i32) -> Option<u64> {
        self.index.lock().unwrap().get(&(dim, cx, cz)).map(|e| e.time_ms)
    }
}

/// `.rcc` ヘッダ先頭 28 バイトから time_ms だけを取り出す (LRU 索引の再構築用)。
/// magic/version 不一致・サイズ不足・I/O 失敗は None → 呼び出し側で最古 (0) 扱い。
fn read_header_time_ms(path: &Path) -> Option<u64> {
    let mut head = [0u8; 28];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if u32::from_le_bytes(head[0..4].try_into().unwrap()) != RCC_MAGIC {
        return None;
    }
    if u32::from_le_bytes(head[4..8].try_into().unwrap()) != RCC_VERSION {
        return None;
    }
    Some(u64::from_le_bytes(head[20..28].try_into().unwrap()))
}

/// "12_-34.rcc" → (12, -34)。`{cx}_{cz}` ちょうど 2 セグメントのみ受理する。
fn parse_chunk_name(p: &Path) -> Option<(i32, i32)> {
    let stem = p.file_stem()?.to_str()?;
    let mut it = stem.split('_');
    let cx = it.next()?.parse().ok()?;
    let cz = it.next()?.parse().ok()?;
    if it.next().is_some() {
        // "1_2_3.rcc" のような形式外ファイルは索引に載せない。
        return None;
    }
    Some((cx, cz))
}

fn zstd_encode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    zstd::stream::encode_all(std::io::Cursor::new(data), 13)
}

fn zstd_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    zstd::stream::read::Decoder::new(std::io::Cursor::new(data))
        .and_then(|mut d| d.read_to_end(&mut out))?;
    Ok(out)
}

/// IEEE CRC32 (MSB-reflected 0xEDB8_cool polynomial)。
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rcc_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rcc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = BobbyCache::new(&dir, "127.0.0.1:25565", 1 << 22).unwrap();
        let chunk = CachedChunk {
            dim: 0,
            cx: 12,
            cz: -34,
            time_ms: 123456789,
            payload: vec![7u8; 65_536],
        };
        cache.store(&chunk).unwrap();
        let got = cache.load(0, 12, -34).unwrap().expect("payload");
        assert_eq!(got.payload, chunk.payload);
        assert_eq!(got.time_ms, chunk.time_ms);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_crc_detected() {
        let dir = std::env::temp_dir().join(format!("rcc_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = BobbyCache::new(&dir, "s", 1 << 20).unwrap();
        let chunk = CachedChunk { dim: 1, cx: 0, cz: 0, time_ms: 0, payload: vec![1u8; 100] };
        cache.store(&chunk).unwrap();
        let path = cache.chunk_path(1, 0, 0);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x55;
        std::fs::write(&path, bytes).unwrap();
        assert!(cache.load(1, 0, 0).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_names() {
        assert_eq!(
            parse_chunk_name(Path::new("/tmp/12_-34.rcc")),
            Some((12, -34))
        );
        assert_eq!(parse_chunk_name(Path::new("/tmp/0_0.rcc")), Some((0, 0)));
    }

    /// 決定的疑似乱数 (LCG)。zstd が実効的に圧縮できない高エントロピー列を
    /// 完全に再現可能な形で生成する (テストの決定性を保つため乱数器は使わない)。
    fn noise_bytes(seed: u64, n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        let mut s = seed;
        for _ in 0..n {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            v.push((s >> 33) as u8);
        }
        v
    }

    #[test]
    fn rebuild_restores_time_ms_from_header() {
        // rebuild_index がヘッダの time_ms を復元することを直接検証する。
        // 0 埋め実装では rebuild 後は全件 Some(0) となり必ず失敗する。
        let dir = std::env::temp_dir().join(format!("rcc_rebuild_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let cache = BobbyCache::new(&dir, "s", 1 << 20).unwrap();
            for (i, t) in [(1, 111u64), (2, 222), (3, 333)] {
                cache
                    .store(&CachedChunk {
                        dim: 0,
                        cx: i,
                        cz: 0,
                        time_ms: t,
                        payload: vec![i as u8; 64],
                    })
                    .unwrap();
            }
            // store 直後 (rebuild 前) の索引は当然に正しい — まず sanity check。
            assert_eq!(cache.index_time_ms(0, 1, 0), Some(111));
            assert_eq!(cache.cached_count(), 3);
        }
        // プロセス再起動相当: index 再構築後も時刻が保たれること。
        let cache = BobbyCache::new(&dir, "s", 1 << 20).unwrap();
        assert_eq!(cache.cached_count(), 3);
        assert_eq!(cache.index_time_ms(0, 1, 0), Some(111));
        assert_eq!(cache.index_time_ms(0, 2, 0), Some(222));
        assert_eq!(cache.index_time_ms(0, 3, 0), Some(333));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lru_after_rebuild_evicts_true_oldest() {
        // 修正前 (時刻復元なし) は rebuild 後に全件 time_ms=0 となり、
        // tie-break (dim,cx,cz) で「座標が最小のエントリ」が真っ先に消える。
        // ここでは座標を時刻に逆相関させてある (t=100→座標9 … t=300→座標1) ので、
        // 修正前は「最新 t=300 が消える」ことになり決定的に失敗し、
        // 修正後のみ「真の最古 t=100 から消えて t=300 が残る」。
        let dir = std::env::temp_dir().join(format!("rcc_lru_re_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            // 16MiB 上限: purge されず 3 件とも保持させる。
            let cache = BobbyCache::new(&dir, "s", 1 << 24).unwrap();
            for (cx, t) in [(9, 100u64), (5, 200), (1, 300)] {
                cache
                    .store(&CachedChunk {
                        dim: 0,
                        cx,
                        cz: cx,
                        time_ms: t,
                        payload: noise_bytes(t, 600_000),
                    })
                    .unwrap();
            }
            assert_eq!(cache.cached_count(), 3);
        }
        // ~600KB × 3 ≒ 1.8MB > 1MiB の上限で開き直し、手動 purge。
        let cache = BobbyCache::new(&dir, "s", 1 << 20).unwrap();
        cache.enforce_lru().unwrap();
        assert!(
            cache.load(0, 9, 9).unwrap().is_none(),
            "最古 t=100 が消える"
        );
        assert!(
            cache.load(0, 5, 5).unwrap().is_none(),
            "次に古い t=200 も消える (残り ~600KB ≤ 1MiB で停止)"
        );
        let newest = cache
            .load(0, 1, 1)
            .unwrap()
            .expect("最新 t=300 は残る");
        assert_eq!(newest.payload, noise_bytes(300, 600_000));
        assert_eq!(newest.time_ms, 300);
        assert_eq!(cache.cached_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_ignores_foreign_dim_dirs() {
        // 数値でない次元ディレクトリ名 (外部混入のゴミ) を dim=0 として
        // 索引に混ぜる実装では cached_count が 2 に化けて決定的に失敗する。
        let dir = std::env::temp_dir().join(format!("rcc_dimskip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = BobbyCache::new(&dir, "s", 1 << 20).unwrap();
        cache
            .store(&CachedChunk {
                dim: 2,
                cx: 1,
                cz: 1,
                time_ms: 5,
                payload: vec![9u8; 32],
            })
            .unwrap();
        let junk = dir
            .join("bobby_cache")
            .join(format!("{:016x}", cache.server_key()))
            .join("junk");
        std::fs::create_dir_all(&junk).unwrap();
        std::fs::write(junk.join("7_7.rcc"), b"not-a-real-chunk").unwrap();
        cache.rebuild_index().unwrap();
        assert_eq!(cache.cached_count(), 1, "junk 次元は索引に入らない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_chunk_name_strict_two_segments() {
        assert_eq!(parse_chunk_name(Path::new("/x/1_2.rcc")), Some((1, 2)));
        assert_eq!(parse_chunk_name(Path::new("/x/-1_-2.rcc")), Some((-1, -2)));
        // 形式外 (3 セグメント・空セグメント・非数値) は索引に載せない。
        assert_eq!(parse_chunk_name(Path::new("/x/1_2_3.rcc")), None);
        assert_eq!(parse_chunk_name(Path::new("/x/1_.rcc")), None);
        assert_eq!(parse_chunk_name(Path::new("/x/_2.rcc")), None);
        assert_eq!(parse_chunk_name(Path::new("/x/a_b.rcc")), None);
    }
}
