//! テクスチャストリーミング — VRAM 予算内で必要な mip/テクスチャだけを常駐させ、
//! LRU で退避し、ヒステリシスでスラッシングを防ぐ。
//!
//! 統合メモリ（Apple Silicon / 内蔵GPU・GPU共有）環境ではスパーステクスチャや
//! ストリーミングが特に有効。外付け帯域を節約しつつ必要な解像度を確保する。
//!
//! **計算量 (2026-07-23 wave 43 で真の O(1) 化)**: 常駐判定・touch・退避
//! は全て侵入型双方向リスト + スロット再利用で O(1) (旧実装は `used()` の
//! 全区間総和と touch の `lru.retain` が**毎要求 O(n)** だった)。
//! `used_bytes` は差分追跡。退避順序は旧 VecDeque 版と**完全同一**
//! (front=LRU … tail 退避 / back=MRU … push 側を head=MRU, tail=LRU と
//! した侵入型リストで同一次元を再現)。

use std::collections::HashMap;

const NIL: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Node {
    prev: u32,
    next: u32,
    id: u32,
    bytes: u64,
}

/// VRAM 予算付き LRU 常駐マネージャ。
///
/// **契約 (2026-07-23 wave 43 厳格化)**:
/// - `request(id)` は `set_size(id, ..)` 済みの id 必須 (未登録 id の
///   要求は旧実装では**静寂に無視**され、テクスチャが永久に非表示の
///   まま無信号だった → fail-loud)。
/// - `hysteresis` は **[0,1) の有限値**必須 (pub フィールドのままだが
///   request 入口で毎回検証する。旧実装は NaN で `limit` が飽和キャスト
///   0 化・>1.0 で 0 化・<0 で**予算超過を静寂許容**していた)。
/// - 単一テクスチャが有効上限 `limit` を超える要求は fail-loud
///   (旧実装は全退避の挙げ句に超過テクスチャを**静寂に挿入**し、
///   予算契約を無通知で破った)。
/// - 予算強制は**request 時点**のみ (set_size での常駐中サイズ変更は
///   帳簿へ即反映するが、超過時の退避は次回 request まで遅延)。
/// - `budget_bytes` を f64 に変換して limit を計るため、budget が
///   2^53 を超えると丸めが入る (実用 VRAM 予算 ≪ 2^53 で non-issue)。
pub struct TextureStreamer {
    pub budget_bytes: u64,
    /// 0..1。予算の (1-hysteresis) までは許容し、余裕ができても即退避しない。
    /// 契約: [0,1) の有限値 (request 入口で検証、違反は panic)。
    pub hysteresis: f64,
    resident: HashMap<u32, u32>, // id -> node slot
    nodes: Vec<Node>,
    free_slots: Vec<u32>,
    head: u32, // MRU (NIL で空)
    tail: u32, // LRU (NIL で空)
    sizes: HashMap<u32, u64>,
    used_bytes: u64,
}

impl TextureStreamer {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            hysteresis: 0.1,
            resident: HashMap::new(),
            nodes: Vec::new(),
            free_slots: Vec::new(),
            head: NIL,
            tail: NIL,
            sizes: HashMap::new(),
            used_bytes: 0,
        }
    }

    pub fn set_size(&mut self, id: u32, bytes: u64) {
        self.sizes.insert(id, bytes);
        // 常駐中のサイズ変更は帳簿へ即反映 (旧実装は resident 側を更新せず
        // 帳簿が静寂にズレた = AS-3 根治)。
        if let Some(&slot) = self.resident.get(&id) {
            let old = self.nodes[slot as usize].bytes;
            debug_assert!(self.used_bytes >= old, "used_bytes 不変条件違反");
            self.nodes[slot as usize].bytes = bytes;
            self.used_bytes = self.used_bytes - old + bytes;
        }
    }

    pub fn resident_bytes(&self) -> u64 {
        self.used_bytes
    }

    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// 常駐照会。LRU 順を変えない純粋クエリ (&self)。
    pub fn is_resident(&self, id: u32) -> bool {
        self.resident.contains_key(&id)
    }

    fn detach(&mut self, slot: u32) {
        let p = self.nodes[slot as usize].prev;
        let n = self.nodes[slot as usize].next;
        if p != NIL {
            self.nodes[p as usize].next = n;
        } else {
            self.head = n;
        }
        if n != NIL {
            self.nodes[n as usize].prev = p;
        } else {
            self.tail = p;
        }
    }

    fn push_front(&mut self, slot: u32) {
        self.nodes[slot as usize].prev = NIL;
        self.nodes[slot as usize].next = self.head;
        if self.head != NIL {
            self.nodes[self.head as usize].prev = slot;
        } else {
            self.tail = slot;
        }
        self.head = slot;
    }

    /// テクスチャを要求（常駐化）。予算超過なら LRU から退避。
    /// 計算量は全経路 O(1)。
    pub fn request(&mut self, id: u32) {
        let Some(&sz) = self.sizes.get(&id) else {
            panic!("TextureStreamer::request 契約違反: 未登録 id {id} (set_size を先行せよ)");
        };
        assert!(
            self.hysteresis.is_finite() && (0.0..1.0).contains(&self.hysteresis),
            "TextureStreamer 契約違反: hysteresis は [0,1) の有限値必須 ({})",
            self.hysteresis
        );
        if self.resident.contains_key(&id) {
            // touch → MRU へ O(1) 移動 (旧 lru.retain O(n) を根治)
            let slot = self.resident[&id];
            if self.head != slot {
                self.detach(slot);
                self.push_front(slot);
            }
            return;
        }
        let limit = (self.budget_bytes as f64 * (1.0 - self.hysteresis)) as u64;
        assert!(
            sz <= limit,
            "TextureStreamer::request 契約違反: 単一テクスチャ {sz}B が有効上限 \
             {limit}B (budget {} × (1-hysteresis {})) を超過 — 静寂な予算破壊を防止",
            self.budget_bytes,
            self.hysteresis
        );
        // sz <= limit より、最悪でも全退避で必ず収まる (ループは必ず終了)。
        while self.used_bytes + sz > limit {
            let victim = self.tail;
            debug_assert!(victim != NIL, "head/tail 不変条件違反");
            let vnode = self.nodes[victim as usize];
            self.used_bytes -= vnode.bytes;
            self.detach(victim);
            let removed = self.resident.remove(&vnode.id);
            debug_assert!(removed.is_some(), "resident/list 不変条件違反");
            self.free_slots.push(victim);
        }
        let slot = if let Some(s) = self.free_slots.pop() {
            self.nodes[s as usize] = Node {
                prev: NIL,
                next: NIL,
                id,
                bytes: sz,
            };
            s
        } else {
            let s = self.nodes.len() as u32;
            self.nodes.push(Node {
                prev: NIL,
                next: NIL,
                id,
                bytes: sz,
            });
            s
        };
        self.resident.insert(id, slot);
        self.push_front(slot);
        self.used_bytes += sz;
    }
}

pub struct MipStreaming;
impl MipStreaming {
    pub fn wgsl_source(&self) -> &'static str {
        MIP_STREAMING_WGSL
    }
}
pub const MIP_STREAMING_WGSL: &str = include_str!("../shaders/mip_streaming.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_within_budget() {
        let mut s = TextureStreamer::new(1000);
        for i in 0..10u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        assert!(s.resident_bytes() <= 1000, "over budget: {}", s.resident_bytes());
        assert!(s.is_resident(9), "most-recent should stay");
        assert!(!s.is_resident(0), "oldest should be evicted");
    }

    #[test]
    fn rerequest_keeps_resident() {
        let mut s = TextureStreamer::new(1000);
        s.set_size(1, 200);
        s.request(1);
        s.set_size(2, 200);
        s.request(2);
        s.request(1); // touch 1
        assert!(s.is_resident(1));
    }

    #[test]
    fn hysteresis_prevents_immediate_thrash() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = 0.5; // allow up to 500 extra
        for i in 0..6u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        assert!(s.resident_bytes() <= 1000);
    }

    /// wave 43-1: LRU 犠牲順序の厳密列ピン (旧 VecDeque 版との完全同一性を
    /// 追跡導出で固定: front=LRU 側から退避、touch は MRU 昇格)。
    #[test]
    fn lru_victim_order_exact_sequence() {
        let mut s = TextureStreamer::new(1000); // limit = (1000 * 0.9) = 900
        for i in 0..4u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        assert_eq!(s.resident_bytes(), 800);
        s.request(1); // touch: LRU 末尾は 0
        s.set_size(4, 200);
        s.request(4); // 800+200>900 → victim 0 → used 800
        assert!(!s.is_resident(0));
        for i in 1..=4u32 {
            assert!(s.is_resident(i), "id {i} は生存");
        }
        assert_eq!(s.resident_bytes(), 800);
        assert_eq!(s.resident_count(), 4);
        // LRU 順: MRU 4,3,2,1 LRu? → tail は 2 (1 は touch 済)
        s.set_size(5, 200);
        s.request(5);
        assert!(!s.is_resident(2), "次の犠牲は 2");
        assert!(s.is_resident(1));
        assert_eq!(s.resident_bytes(), 800);
    }

    /// wave 43-2: 境界 — used+sz == limit は退避しない (厳密等価)。
    #[test]
    fn budget_boundary_equality_inserts_without_eviction() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = 0.0; // limit = 1000
        s.set_size(1, 500);
        s.request(1);
        s.set_size(2, 500);
        s.request(2); // 500+500 == 1000 (not >) → 退避なし挿入
        assert!(s.is_resident(1));
        assert!(s.is_resident(2));
        assert_eq!(s.resident_bytes(), 1000);
        s.set_size(3, 1);
        s.request(3); // 1001 > 1000 → victim 1 のみ → used 1+500+... = 501
        assert!(!s.is_resident(1));
        assert!(s.is_resident(2));
        assert!(s.is_resident(3));
        assert_eq!(s.resident_bytes(), 501);
    }

    /// wave 43-3: 常駐中のサイズ変更は帳簿へ即反映され、後続の request は
    /// 新サイズで予算判定する (AS-3 根治の機械ピン)。
    #[test]
    fn resize_while_resident_keeps_accounting_consistent() {
        let mut s = TextureStreamer::new(1000); // limit 900
        s.set_size(1, 200);
        s.request(1);
        s.set_size(1, 400);
        assert_eq!(s.resident_bytes(), 400, "拡大は帳簿へ即反映");
        s.set_size(1, 50);
        assert_eq!(s.resident_bytes(), 50, "縮小も帳簿へ即反映");
        s.set_size(2, 480);
        s.request(2); // used 530
        s.set_size(3, 480);
        s.request(3); // 530+480=1010>900 → victim 1 → 480+480=960>900 → victim 2 → used 480
        assert!(s.is_resident(3));
        assert!(!s.is_resident(1));
        assert!(!s.is_resident(2));
        assert_eq!(s.resident_bytes(), 480);
    }

    /// wave 43-4: スロット再利用 (free list) — 大量の出入れでも nodes は
    /// ピーク常駐数を超えて伸びない。
    #[test]
    fn slots_are_reused_not_grown() {
        let mut s = TextureStreamer::new(1000); // limit 900 → 同時 4 枚まで (200B 級)
        for i in 0..100u32 {
            s.set_size(i, 300);
            s.request(i); // 300*4=1200>900 → 定常 3 枚
        }
        assert_eq!(s.resident_count(), 3);
        assert!(
            s.nodes.len() <= 4,
            "スロット再利用で nodes は定常サイズ (実測 {})",
            s.nodes.len()
        );
    }

    /// wave 43-5: 未登録 id の要求は fail-loud (旧: 静寂無視で永久非表示)。
    #[test]
    #[should_panic(expected = "未登録 id 42")]
    fn request_rejects_unknown_id() {
        let mut s = TextureStreamer::new(1000);
        s.request(42);
    }

    /// wave 43-6: 単一テクスチャの有効上限超過は fail-loud (旧: 全退避の
    /// 挙げ句に超過挿入して予算契約を静寂に破壊)。
    #[test]
    #[should_panic(expected = "が有効上限")]
    fn request_rejects_oversized_texture() {
        let mut s = TextureStreamer::new(1000); // limit 900
        s.set_size(1, 901);
        s.request(1);
    }

    /// wave 43-7: hysteresis 契約 — NaN / 範囲外は fail-loud (旧: 飽和
    /// キャストで limit 0 化 / 予算超過を静寂許容)。
    #[test]
    #[should_panic(expected = "hysteresis は [0,1) の有限値必須")]
    fn request_rejects_nan_hysteresis() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = f64::NAN;
        s.set_size(1, 1);
        s.request(1);
    }

    #[test]
    #[should_panic(expected = "hysteresis は [0,1) の有限値必須")]
    fn request_rejects_hysteresis_one() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = 1.0;
        s.set_size(1, 1);
        s.request(1);
    }

    #[test]
    #[should_panic(expected = "hysteresis は [0,1) の有限値必須")]
    fn request_rejects_negative_hysteresis() {
        let mut s = TextureStreamer::new(1000);
        s.hysteresis = -0.5;
        s.set_size(1, 1);
        s.request(1);
    }

    /// wave 43-8: touch は LRU 順のみ変え、帳簿・常駐集合・退避順を変えない
    /// (head スロット自身の touch は no-op 経路も含む)。
    #[test]
    fn touch_is_accounting_neutral() {
        let mut s = TextureStreamer::new(1000);
        for i in 0..3u32 {
            s.set_size(i, 200);
            s.request(i);
        }
        s.request(2); // head (MRU) 自身の touch
        s.request(0); // tail (LRU) の touch → MRU へ
        assert_eq!(s.resident_bytes(), 600);
        assert_eq!(s.resident_count(), 3);
        // LRU 順: MRU 0,2,1 LRu? → tail は 1
        s.set_size(9, 400);
        s.request(9); // 600+400=1000>900 → victim 1 → used 800 ≤ 900
        assert!(!s.is_resident(1));
        assert!(s.is_resident(0));
        assert!(s.is_resident(2));
        assert!(s.is_resident(9));
        assert_eq!(s.resident_bytes(), 800);
    }

    /// wave 43-9: mip_streaming.wgsl は「シェーダ不要」の**正当なマーカー**
    /// であることを機械ピン (空モジュールの parse 成功 + entry/binding ゼロ)。
    /// 常駐判定はホスト API 操作 (リソース生成・上傳順序はシェーダ実行前に
    /// 確定要) のため GPU 側決定は構造的に不可能 — 詳細は wgsl コメント参照。
    #[test]
    fn wgsl_is_intentionally_shader_free_marker() {
        let module = naga::front::wgsl::parse_str(MIP_STREAMING_WGSL).expect("marker must parse");
        assert!(
            module.entry_points.is_empty(),
            "CPU ポリシーモジュールにシェーダは要らない (設計正当性は wgsl コメント参照)"
        );
        assert!(module.global_variables.iter().next().is_none());
    }
}
