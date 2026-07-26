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
//!
//! 【契約注記 (監査 2026-07-26 DO 節)】
//! * **部分書換えは残部を潰さない**: `data` が 64 KiB 未満の場合、ページの
//!   残りバイトは以前の内容のまま残る。剥奪で譲受したページの末尾には
//!   **旧占有チャンクの残滓**が残存し、`out` を長めに渡した呼出側は
//!   その残滓を読み得る (生ストレージ設計: 長さ管理は呼出側の責務)。
//!   現行の唯一消費者 full_graph_wiring:747 は 8 byte 書込みのみで
//!   **read 側を一切呼ばない**ため現時点で残留バイトの観測者は存在しない
//!   (将来の読出し実装時に本契約を再確認すること、strict pin 済)。
//! * `read_chunk_page` の `Ok(0)` は 2 つの意味を持つ: 未登録キー、
//!   または `out` が空 (登録済みでも 0 返却)。区別は `page_table.contains_key`。
//! * **再起動は再装着しない**: `new()` は page_table を空で開始し、
//!   ファイルの既存内容は物理的には残るが**到達不能**になる (orphan)。
//!   また旧ファイルが cap より大きい場合 `set_len` で切り詰める。
//! * I/O 毎に `File` を再オープンする (fd 非キャッシュ設計)。エラーは全て
//!   `Err(String)` 返却で fail-visible (panic 経路なし)。

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
        let file = OpenOptions::new()
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

    /// 書込み。`data` が PAGE_SIZE_BYTES (64 KiB) を超えても先頭 64 KiB のみ
    /// 保存し (上げ落ち)、未満なら残部は**無改変** (旧占有者の残滓が残ること
    /// あり、ヘッダ契約注記 DO 参照)。
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
    /// 未登録 (剥奪済み含む) は Ok(0)。**Ok(0) は多義**: out 空でも Ok(0)。
    /// out は PAGE_SIZE_BYTES で打止め (隣接ページ侵入なし) が、ページ内の
    /// 書込み長を超えた領域はファイル現状のまま (旧占有者の残滓含む) を返す。
    /// 長さ帳簿は呼出側責務 (ヘッダ契約注記 DO 参照)。
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
        // D 投入: read 最新化が被害者を決定的に変える。最古は C
        // (write B→page1, C→page0(A 譲受) のため page1 = B が生存)。
        let hd = paging.write_chunk_page(4, 4, &d).unwrap();
        assert_eq!(hd.page_idx, 0, "B の read 最新化により C が最古 → page0 を譲受");
        assert_eq!(paging.read_chunk_page(3, 3, &mut staging).unwrap(), 0);
        assert_eq!(paging.read_chunk_page(2, 2, &mut buf).unwrap(), 100);
        assert_eq!(buf, b, "read 最新化により B は生存 (page1 据置き)");
        // D は C の page0 を占有しており、内容は厳密に D のもの。
        assert_eq!(paging.read_chunk_page(4, 4, &mut buf).unwrap(), 100);
        assert_eq!(buf, d, "page0 は D の内容 (C の残滓は上書き済み)");
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

    // ------------------------------------------------- 監査 2026-07-26 DO 追加分

    /// DO-2: 部分書換えは尾を潰さない — 剥奪で譲受したページには
    /// **旧占有者の残滓**が残り、長めの out で読み得る (設計: 長さ帳簿は
    /// 呼出側責務。誇張せず「現消費者 wiring は 8B 書きのみで read 未使用」
    /// のため観測者不在であることも明記)。読出しはページ寸法で打止め。
    #[test]
    fn partial_write_leaves_stale_tail_contract_pin() {
        let temp = unique_path("tail");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 1).unwrap();
        let full = [0xA1u8; PAGE_SIZE_BYTES];
        let ha = paging.write_chunk_page(10, 10, &full).unwrap();
        assert_eq!(ha.page_idx, 0, "容量 1 で A→page0");
        // D を 100 byte だけ書く → A が剥奪されて page0 を譲受。
        let d = [0xD7u8; 100];
        let hd = paging.write_chunk_page(20, 20, &d).unwrap();
        assert_eq!(hd.page_idx, 0, "D は A 剥奪の page0 を譲受");
        // 256 byte 要求: 先頭 100 は D、その後ろは A の残滓 0xA1 のまま。
        let mut out = [0u8; 256];
        let got = paging.read_chunk_page(20, 20, &mut out).unwrap();
        assert_eq!(got, 256, "要求長のまま (ページ打止めより内側)");
        assert_eq!(&out[..100], &d[..], "先頭 100 は D 厳密");
        assert!(
            out[100..256].iter().all(|&b| b == 0xA1),
            "書込み長を超える領域は旧占有者 A の残滓 (ゼロ潰ししない契約)"
        );
        let _ = std::fs::remove_file(temp);
    }

    /// DO-3: read の Ok(0) は 2 義 (未登録キー / 登録済みだが out 空)。
    #[test]
    fn read_ok0_is_ambiguous_contract_pin() {
        let temp = unique_path("ok0");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 4).unwrap();
        paging.write_chunk_page(1, 1, &[1u8, 2, 3, 4]).unwrap();
        // 未登録キー → Ok(0)、out は無改変のまま。
        let mut probe = [0x55u8; 8];
        assert_eq!(paging.read_chunk_page(99, 99, &mut probe).unwrap(), 0);
        assert_eq!(probe, [0x55u8; 8], "未登録読出しは out を触らない");
        // 登録済み + out 空 → これも Ok(0) (多義の pin)。
        assert_eq!(paging.read_chunk_page(1, 1, &mut []).unwrap(), 0);
        // 区別は contains_key で行うこと (契約)。
        assert!(paging.page_table.contains_key(&(1, 1)));
        assert!(!paging.page_table.contains_key(&(99, 99)));
        let _ = std::fs::remove_file(temp);
    }

    /// DO-4: 真 LRU の被害者選択と page_idx 割当を、独立実装のシャドウ
    /// モデル (再現ロジックを別形態で書いた参照) と 1,000 オペ差分照合。
    /// structural invariant (page_table ⇔ lru_order の 1:1) も併せて pin。
    #[test]
    fn shadow_model_differential_fuzz_lru() {
        #[derive(Default)]
        struct Shadow {
            cap: usize,
            lru: Vec<(i32, i32)>,
            idx: std::collections::HashMap<(i32, i32), u32>,
            next: u32,
        }
        impl Shadow {
            fn write(&mut self, k: (i32, i32)) -> u32 {
                if self.idx.contains_key(&k) {
                    self.lru.retain(|&x| x != k);
                    self.lru.push(k);
                    return self.idx[&k];
                }
                let i = if self.idx.len() < self.cap {
                    let i = self.next;
                    self.next += 1;
                    i
                } else {
                    let victim = self.lru.remove(0);
                    self.idx.remove(&victim).unwrap()
                };
                self.idx.insert(k, i);
                self.lru.push(k);
                i
            }
            fn read(&mut self, k: (i32, i32)) -> bool {
                if self.idx.contains_key(&k) {
                    self.lru.retain(|&x| x != k);
                    self.lru.push(k);
                    true
                } else {
                    false
                }
            }
        }
        let temp = unique_path("shadow");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 5).unwrap();
        let mut shadow = Shadow {
            cap: 5,
            ..Default::default()
        };
        const K: i32 = 12; // キー候補数 (cap 5 を上回る=)
        let mut s = 0xC0FFEE_u64;
        let mut rng = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let payload_of = |k: (i32, i32)| [((k.0 * 31 + k.1 * 17) & 0xFF) as u8; 40];
        let mut rdbuf = [0u8; 40];
        for op in 0..1_000u32 {
            let key = ((rng() % K as u64) as i32, (rng() % K as u64) as i32);
            if rng() % 10 < 7 {
                // write 7 割
                let want = payload_of(key);
                let got = paging.write_chunk_page(key.0, key.1, &want).unwrap();
                let expect = shadow.write(key);
                assert_eq!(
                    got.page_idx, expect,
                    "op {op}: write 被害者選択/割当がシャドウと不一致"
                );
                assert_eq!(got.offset, expect as usize * PAGE_SIZE_BYTES);
            } else {
                // read 3 割
                rdbuf.iter_mut().for_each(|b| *b = 0xEE);
                let expect = shadow.read(key);
                let got = paging.read_chunk_page(key.0, key.1, &mut rdbuf).unwrap();
                if expect {
                    assert_eq!(got, 40, "op {op}: 登録済みキーは payload 長を返す");
                    assert_eq!(rdbuf, payload_of(key), "op {op}: 内容厳密");
                } else {
                    assert_eq!(got, 0, "op {op}: 未登録は Ok(0)");
                    assert_eq!(rdbuf, [0xEEu8; 40], "op {op}: out 無改変");
                }
            }
            if op % 50 == 0 {
                // structural invariant: page_table と lru_order は常に 1:1。
                assert_eq!(
                    paging.page_table.len(),
                    paging.lru_order.len(),
                    "op {op}: page_table/lru_order 件数分裂"
                );
                let mut a: Vec<_> = paging.page_table.keys().copied().collect();
                let mut b: Vec<_> = paging.lru_order.iter().copied().collect();
                a.sort();
                b.sort();
                assert_eq!(a, b, "op {op}: page_table/lru_order のキー集合分裂");
            }
        }
        let _ = std::fs::remove_file(temp);
    }

    /// DO-5: idx 上限・offset 算術・バッキング長・再起動 orphan の pin。
    #[test]
    fn bounds_offset_backing_and_restart_orphan_pin() {
        let temp = unique_path("bounds");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 8).unwrap();
        // 40 キー投入 (cap 8 を超え剥奪反復)。全 handle は idx < 8。
        for i in 0..40i32 {
            let h = paging.write_chunk_page(i, i * 3, &[i as u8; 24]).unwrap();
            assert!(h.page_idx < 8, "剥奪も含め idx は容量未満: {}", h.page_idx);
            assert_eq!(h.offset, h.page_idx as usize * 65536, "offset 算術");
        }
        // バッキングファイル長は cap_bytes そのもの (伸縮しない)。
        let meta = std::fs::metadata(&temp).unwrap();
        assert_eq!(meta.len(), 8 * 65536, "set_len(cap) のみでロック");
        // u32 境界の拒否 (guard は open 前 = 大容量ファイルを作らない)。
        let err = OutOfCoreMmapPaging::new(&temp, u32::MAX as usize + 1).unwrap_err();
        assert!(!err.is_empty(), "u32 超過は理由付きで拒否");
        // 再起動 orphan: (7,7) を書き、new() し直しても再装着しない。
        let mut p1 = OutOfCoreMmapPaging::new(&temp, 8).unwrap();
        p1.write_chunk_page(7, 7, &[0xA5u8; 64]).unwrap();
        drop(p1);
        let mut p2 = OutOfCoreMmapPaging::new(&temp, 8).unwrap();
        let mut out = [0u8; 64];
        assert_eq!(
            p2.read_chunk_page(7, 7, &mut out).unwrap(),
            0,
            "再起動は page_table を空で開始: 既存内容は物理残存するが到達不能"
        );
        let _ = std::fs::remove_file(temp);
    }
}
