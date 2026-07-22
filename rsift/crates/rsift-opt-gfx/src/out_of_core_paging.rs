//! # 25. Out-of-Core Paging (`OutOfCoreMmapPaging` / ファイルページ + 真 LRU)
//!
//! 物理メモリに収まらないワールドのチャンクデータを、64 KiB 固定ページの
//! 単一バッキングファイルへ退避するページャ。
//!
//! 【実装の正直注記 (監査 2026-07-22 L 節)】
//! 実体は memmap2 ではなく `File` + seek/read/write のプレーンなファイル I/O
//! である (型名の `Mmap` は旧来の名残で、メモリマップは一切使用していない)。
//! 置き換えポリシは**真 LRU**: 読み書きのたび対象キーを最新化し、容量到達時は
//! 最古キーのページを剥奪する。旧実装はラウンドロビン回送かつ剥奪された旧キーの
//! `page_table` エントリを除去しなかったため、`max_pages` を超えて別チャンクを
//! 書き込むと**新旧 2 チャンクが同一物理ページを共有し、古い側の読み出しが
//! サイレントに他チャンクの内容を返す破壊的バグ**があった (L-1)。
//! また `max_pages == 0` は剰余ゼロパニックだった (L-2)。
//! 容量内のページ割当て列 (0,1,2,...) は旧実装と同一で、既存呼出側の
//! ビット互換性は保たれる。

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub const PAGE_SIZE_BYTES: usize = 65536; // 64 KB per chunk page

#[derive(Debug, Clone, Copy)]
pub struct PageHandle {
    pub page_idx: u32,
    pub offset: usize,
}

#[derive(Debug)]
pub struct OutOfCoreMmapPaging {
    pub file_path: PathBuf,
    pub page_table: HashMap<(i32, i32), PageHandle>,
    pub max_pages: usize,
    /// 次の未使用ページ。容量未満の間は単調増加 (回送しない — L-1)。
    next_page_idx: u32,
    /// LRU 順序 (back = 直近使用 / front = 最古)。page_table と 1:1 不変条件。
    /// 走査は O(len) だが max_pages (数千) と呼出頻度 (4 tick 毎) では実害なし。
    lru_order: VecDeque<(i32, i32)>,
}

impl OutOfCoreMmapPaging {
    pub fn new(file_path: impl Into<PathBuf>, max_pages: usize) -> Result<Self, String> {
        // L-2: 0 ページは全 write が置き換え不能 (旧実装は剰余ゼロパニック)。
        if max_pages == 0 {
            return Err("max_pages == 0 はページング不能のため拒否 (旧: 剰余ゼロパニック)".into());
        }
        // page_idx は u32: 4G ページ超は表現不能 (256 TiB 級、現実非到達だが明示拒否)。
        if (max_pages as u128) > u32::MAX as u128 {
            return Err("max_pages が u32 ページ index の表現域を超過".into());
        }
        let cap_bytes = max_pages
            .checked_mul(PAGE_SIZE_BYTES)
            .ok_or_else(|| "max_pages * PAGE_SIZE_BYTES が usize を溢れる".to_string())?;
        let path = file_path.into();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        // Pre-allocate initial backing store
        file.set_len(cap_bytes as u64)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            file_path: path,
            page_table: HashMap::new(),
            max_pages,
            next_page_idx: 0,
            lru_order: VecDeque::new(),
        })
    }

    /// 使用時刻を最新化 (LRU の心臓)。未登録キーは no-op。
    fn touch(&mut self, key: (i32, i32)) {
        if let Some(pos) = self.lru_order.iter().position(|&k| k == key) {
            self.lru_order.remove(pos);
            self.lru_order.push_back(key);
        }
    }

    pub fn write_chunk_page(
        &mut self,
        cx: i32,
        cz: i32,
        data: &[u8],
    ) -> Result<PageHandle, String> {
        let key = (cx, cz);
        let handle = if let Some(&h) = self.page_table.get(&key) {
            // 既存キー: ページ割当ては据え置き、LRU 位置のみ更新。
            self.touch(key);
            h
        } else {
            let idx = if self.page_table.len() < self.max_pages {
                // 容量内: 未使用ページを単調割当て (旧実装と同一の列)。
                let i = self.next_page_idx;
                self.next_page_idx += 1;
                i
            } else {
                // 真 LRU 置き換え: 最古キーのエントリを page_table から
                // **完全除去**してそのページを譲り受ける (旧実装はエントリを
                // 残しエイリアス破壊を引き起こした — L-1)。
                let victim = self
                    .lru_order
                    .pop_front()
                    .ok_or_else(|| "page_table と lru_order の件数不一致".to_string())?;
                let vh = self
                    .page_table
                    .remove(&victim)
                    .ok_or_else(|| "lru_order が未登録キーを指す".to_string())?;
                vh.page_idx
            };
            let handle = PageHandle {
                page_idx: idx,
                offset: idx as usize * PAGE_SIZE_BYTES,
            };
            self.page_table.insert(key, handle);
            self.lru_order.push_back(key);
            handle
        };

        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.file_path)
            .map_err(|e| e.to_string())?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(handle.offset as u64))
            .map_err(|e| e.to_string())?;
        let to_write = data.len().min(PAGE_SIZE_BYTES);
        file.write_all(&data[..to_write]).map_err(|e| e.to_string())?;
        Ok(handle)
    }

    /// 読み出しも LRU 使用として記録する (真 LRU の読み側)。
    /// 未登録 (剥奪済み含む) は Ok(0)。
    pub fn read_chunk_page(&mut self, cx: i32, cz: i32, out: &mut [u8]) -> Result<usize, String> {
        let key = (cx, cz);
        let Some(&handle) = self.page_table.get(&key) else {
            return Ok(0);
        };
        self.touch(key);
        let mut file = File::open(&self.file_path).map_err(|e| e.to_string())?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(handle.offset as u64))
            .map_err(|e| e.to_string())?;
        use std::io::Read;
        let to_read = out.len().min(PAGE_SIZE_BYTES);
        file.read_exact(&mut out[..to_read]).map_err(|e| e.to_string())?;
        Ok(to_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsift_paging_{}_{}_{}.bin",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn roundtrip_exact_bytes_full_compare() {
        let temp = unique_path("roundtrip");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 16).unwrap();
        // ページ全域に決定的パターン (i*7+3 mod 256)。
        let payload: Vec<u8> = (0..PAGE_SIZE_BYTES).map(|i| (i * 7 + 3) as u8).collect();
        let h = paging.write_chunk_page(1, 2, &payload).unwrap();
        assert_eq!(h.page_idx, 0, "容量内の割当て列は 0 から単調 (旧互換)");
        let mut buf = vec![0u8; PAGE_SIZE_BYTES];
        let read = paging.read_chunk_page(1, 2, &mut buf).unwrap();
        assert_eq!(read, PAGE_SIZE_BYTES);
        assert_eq!(buf, payload, "全 64 KiB のビット完全一致");
        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn zero_max_pages_is_err_not_panic() {
        // L-2 回帰: 旧実装は write 時の剰余ゼロでパニック。
        let temp = unique_path("zero");
        let err = OutOfCoreMmapPaging::new(&temp, 0).unwrap_err();
        assert!(!err.is_empty(), "拒否理由が分かるメッセージ");
        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn lru_evicts_oldest_and_never_aliases() {
        // L-1 回帰: 剥奪キーのエントリが残り新旧チャンクがページ共有する旧欠陥。
        let temp = unique_path("lru");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 2).unwrap();
        let a = [0xAAu8; 100];
        let b = [0xBBu8; 100];
        let c = [0xCCu8; 100];
        let d = [0xDDu8; 100];
        paging.write_chunk_page(1, 1, &a).unwrap(); // lru [A]   page0
        paging.write_chunk_page(2, 2, &b).unwrap(); // lru [A,B] page1
        // 容量到達。C 投入で最古 A が剥奪され page0 を譲受。
        let hc = paging.write_chunk_page(3, 3, &c).unwrap(); // lru [B,C]
        assert_eq!(hc.page_idx, 0, "A は front なので page0 を譲受");
        assert_eq!(paging.page_table.len(), 2, "容量維持");
        let mut buf = [0u8; 100];
        let mut staging = [0u8; 100];
        // A は剥奪済み: Ok(0) であり 0xAA/0xCC いずれの混入も無い。
        assert_eq!(
            paging.read_chunk_page(1, 1, &mut staging).unwrap(),
            0,
            "剥奪キーは完全未登録 (旧実装は他チャンクの内容を返した)"
        );
        // read は使用として記録する: B を最新化 → lru [C,B]。
        assert_eq!(paging.read_chunk_page(2, 2, &mut buf).unwrap(), 100);
        assert_eq!(buf, b, "B は生存し内容厳密");
        // D 投入: read 最新化が被害者を決定的に変える。最古は C (page1 所有者)。
        let hd = paging.write_chunk_page(4, 4, &d).unwrap();
        assert_eq!(hd.page_idx, 1, "B の read 最新化により C が最古 → page1 を譲受");
        assert_eq!(paging.read_chunk_page(3, 3, &mut staging).unwrap(), 0);
        assert_eq!(paging.read_chunk_page(2, 2, &mut buf).unwrap(), 100);
        assert_eq!(buf, b, "read 最新化により B は生存");
        // D は C の page1 を占有しており、内容は厳密に D のもの。
        assert_eq!(paging.read_chunk_page(4, 4, &mut buf).unwrap(), 100);
        assert_eq!(buf, d, "page1 は D の内容 (C の残滓は上書き済み)");
        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn oversize_payload_truncates_to_page_and_reads_back() {
        let temp = unique_path("oversize");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 4).unwrap();
        let big: Vec<u8> = (0..(PAGE_SIZE_BYTES + 5000))
            .map(|i| (i as u32).wrapping_mul(2654435761) as u8)
            .collect();
        paging.write_chunk_page(9, 9, &big).unwrap();
        let mut wide = vec![0u8; PAGE_SIZE_BYTES + 5000];
        let got = paging.read_chunk_page(9, 9, &mut wide).unwrap();
        assert_eq!(got, PAGE_SIZE_BYTES, "読み出しはページ寸法で打止め");
        assert_eq!(&wide[..PAGE_SIZE_BYTES], &big[..PAGE_SIZE_BYTES], "保存分は厳密一致");
        let _ = std::fs::remove_file(temp);
    }
}
