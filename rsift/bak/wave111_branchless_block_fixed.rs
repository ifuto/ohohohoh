//! Branchless Block Logic - if(block==water)をLUT + cmovに
//! 低スペCPUの分岐予測ミスペナルティ回避
//!
//! 【正直注記 (2026-07-21 監査)】アクセス経路は実ブランチレス (テーブル参照
//! + cmov 相当算術) だが、**テーブル値自体は決定的プレースホルダ**
//! (i%3 / i%5 / i%16 の modulo ヒューリスティック) であり、ブロックの実属性
//! (実不透明性・実発光レベル) の一次情報テーブルではない。vanilla ハッシュ
//! blockstate id → 実属性の正式マッピングはまだどのクレートにも存在せず、
//! ここが配線されることで消費される full_graph_wiring の「発光シード採取 /
//! opaque 判定」は proxy 精度。テーブル値を変更してはいけない
//! (full_graph_wiring の可視性・ライト出力が変わる) — 正式テーブル化は
//! registry 一次情報が揃った時点で行う将来課題。
//!
//! 【ドメイン契約 (監査 2026-07-25 DK-3)】テーブル幅は 12 bit (4096) 固定。
//! アクセサは `id & 4095` で**静寂 wrap** するため、id ≥ 4096 の入力は
//! 下位 12 bit の別 id へエイリアスされる (u16 全域・vanilla 広域 blockstate
//! 空間の両方が 12 bit を超えうる)。この wrap はプレースホルダ値の仮性とは
//! 独立の第 2 の近似であり、正式テーブル化の際は幅 16 bit 以上の拡張と
//! 同時に解消すべき契約 (full_graph_wiring:986 の u64→u16 キャストを含め
//! 2 段の静寂 truncate が入る経路あり)。
//!
//! 【構造集計 (監査 2026-07-25 DK-4、厳密値 pin 済)】
//! opaque 真 2,730 / 偽 1,366 (i%3!=0)、transparent 真 820 (i%5==0、i=0 含む)、
//! i%15==0 は 274、light の各レベルは完全一様 256 (=4096/16)。
//! **opaque と transparent は排他でない**: i%5==0 かつ i%3!=0 の交差が 546 個
//! 存在 (最小反例 i=10)。「transparent=true なら opaque=false」の仮定を
//! 消費者が置くことは現値では誤り (プレースホルダ由来の非排他)。

pub struct BlockLut {
    pub opaque: [bool; 4096],
    pub transparent: [bool; 4096],
    pub light: [u8; 4096],
}

impl BlockLut {
    pub fn new() -> Self {
        let mut opaque = [false; 4096];
        let mut transparent = [false; 4096];
        let mut light = [0u8; 4096];
        for i in 0..4096 {
            opaque[i] = i % 3 != 0;
            transparent[i] = i % 5 == 0;
            light[i] = (i % 16) as u8;
        }
        Self {
            opaque,
            transparent,
            light,
        }
    }

    #[inline(always)]
    pub fn is_opaque_branchless(&self, id: u16) -> bool {
        // 分岐無し: テーブル参照のみ。id ≥ 4096 は下位 12 bit に wrap (DK-3)。
        self.opaque[id as usize & 4095]
    }

    /// transparent フラグ参照 (消費者ゼロの意図的保持: 将来の半透明ソート/
    /// 透過パス向け accessor 整備。監査 2026-07-25 DK-2)。
    /// **opaque と排他ではない** (DK-4: 交差 546 個、最小反例 i=10)。
    #[inline(always)]
    pub fn transparent_branchless(&self, id: u16) -> bool {
        self.transparent[id as usize & 4095]
    }

    #[inline(always)]
    pub fn light_branchless(&self, id: u16) -> u8 {
        self.light[id as usize & 4095]
    }

    #[inline(always)]
    pub fn select_branchless(cond: bool, a: u32, b: u32) -> u32 {
        // cmov相当: m ∈ {0,1} なので (m*a) と ((1-m)*b) の片側は必ず 0。
        // よってビット OR `|` は加算 `+` と厳密一致 (0|x = 0+x = x) し、
        // a, b がビットを共有しても常に一方だけが厳密に選ばれる。
        let m = cond as u32;
        (m * a) | ((1 - m) * b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_construct_is_deterministic_and_documented_placeholder() {
        // テーブル値は modulo プレースホルダ (ヘッダ正直注記参照) — 変更すると
        // full_graph_wiring の可視性/ライト出力が変わるので、ここで現行値を固定する。
        let a = BlockLut::new();
        let b = BlockLut::new();
        for i in 0..4096usize {
            assert_eq!(a.opaque[i], b.opaque[i]);
            assert_eq!(a.transparent[i], b.transparent[i]);
            assert_eq!(a.light[i], b.light[i]);
        }
        // modulo 規則そのものを pin (id 0 は air で opaque=false / light=0)。
        assert!(!a.opaque[0] && a.light[0] == 0);
        assert!(a.opaque[1] && a.opaque[2]);
        assert!(!a.opaque[3]); // 3%3==0 → placeholder では非不透明
        assert!(a.transparent[10]); // 10%5==0
        assert_eq!(a.light[18], 2); // 18%16
        assert_eq!(a.light[4095], 15);
    }

    #[test]
    fn id_masks_wrap_at_4096() {
        let lut = BlockLut::new();
        for id in [0u16, 1, 2, 3, 255, 1024, 4095] {
            assert_eq!(
                lut.is_opaque_branchless(id),
                lut.is_opaque_branchless(id.wrapping_add(4096)),
                "opaque masking must wrap at 4096"
            );
            assert_eq!(
                lut.light_branchless(id),
                lut.light_branchless(id.wrapping_add(4096))
            );
        }
    }

    #[test]
    fn light_values_never_exceed_15() {
        let lut = BlockLut::new();
        for v in lut.light {
            assert!(v <= 15, "MC light range is 0..=15");
        }
    }

    #[test]
    fn select_branchless_truth_table_including_overlapping_bits() {
        // a, b がビットを共有する場合も cond で厳密に一方だけが選ばれること。
        for (cond, expect) in [(true, 0xDEAD_BEEFu32), (false, 0x0F0F_0F0Fu32)] {
            assert_eq!(
                BlockLut::select_branchless(cond, 0xDEAD_BEEF, 0x0F0F_0F0F),
                expect
            );
        }
        assert_eq!(BlockLut::select_branchless(true, 0, 123), 0);
        assert_eq!(BlockLut::select_branchless(false, 123, 0), 0);
        assert_eq!(BlockLut::select_branchless(true, 7, 7), 7);
    }

    // ------------------------------------------------------------ 監査 2026-07-25 DK追加分

    /// DK-2: transparent accessor はフィールドと wrap を含め完全一致。
    #[test]
    fn transparent_accessor_matches_field_and_wraps() {
        let lut = BlockLut::new();
        for id in 0..4096u16 {
            assert_eq!(lut.transparent_branchless(id), lut.transparent[id as usize]);
        }
        for id in [0u16, 1, 5, 10, 4095] {
            assert_eq!(
                lut.transparent_branchless(id),
                lut.transparent_branchless(id.wrapping_add(4096)),
                "transparent masking must wrap at 4096"
            );
        }
    }

    /// DK-4: 構造集計の厳密値 pin (Python 機械検算と一致:
    /// 2730/1366/820/274/546、light 完全一様 256)。
    #[test]
    fn closed_form_structural_counts_are_exact() {
        let lut = BlockLut::new();
        let op = lut.opaque.iter().filter(|&&b| b).count();
        let tr = lut.transparent.iter().filter(|&&b| b).count();
        let both = (0..4096usize)
            .filter(|&i| lut.opaque[i] && lut.transparent[i])
            .count();
        assert_eq!(op, 2730, "opaque 真 (4096 - ceil(4096/3))");
        assert_eq!(4096 - op, 1366, "opaque 偽");
        assert_eq!(tr, 820, "transparent 真 (floor(4095/5)+1、i=0 含む)");
        assert_eq!(
            both, 546,
            "opaque∧transparent 交差 (820 - 274) = 非排他の証拠"
        );
        // 最小反例: i=10 は opaque (10%3!=0) かつ transparent (10%5==0)。
        assert!(lut.opaque[10] && lut.transparent[10], "非排他の最小反例");
        // light は 16 レベル完全一様 (4096 = 16*256)。
        let mut hist = [0u32; 16];
        for v in lut.light {
            hist[v as usize] += 1;
        }
        assert_eq!(hist, [256u32; 16], "light ヒストグラムは完全一様");
    }

    /// DK-3: id ≥ 4096 の静寂 wrap (エイリアス) 契約の pin。u16 全域で
    /// `id & 4095` の下位 12 bit と必ず一致し、契約変更を検出可能にする。
    #[test]
    fn domain_wrap_contract_holds_for_all_u16() {
        let lut = BlockLut::new();
        for id in 0..=u16::MAX {
            assert_eq!(lut.is_opaque_branchless(id), lut.opaque[id as usize & 4095]);
            assert_eq!(lut.light_branchless(id), lut.light[id as usize & 4095]);
            assert_eq!(
                lut.transparent_branchless(id),
                lut.transparent[id as usize & 4095]
            );
        }
    }

    /// DK-1: select の OR 形は m∈{0,1} では加算形と厳密一致 (片側必ず 0)。
    /// 代表値集合 (全ビット共有・端点・交互パターン) で等価を pin。
    #[test]
    fn select_or_form_equals_plus_form_on_representatives() {
        let vals = [
            0u32,
            1,
            0xDEAD_BEEF,
            0x0F0F_0F0F,
            0xFFFF_FFFF,
            0x8000_0000,
            0xAAAA_AAAA,
            0x5555_5555,
            123_456_789,
            7,
        ];
        for &cond in &[true, false] {
            for &a in &vals {
                for &b in &vals {
                    let m = cond as u32;
                    let or_form = BlockLut::select_branchless(cond, a, b);
                    let plus_form = (m * a) + ((1 - m) * b);
                    assert_eq!(or_form, plus_form, "OR 形と加算形の乖離 (cond={cond})");
                    // 真値仕様: cond なら a、非 cond なら b。
                    assert_eq!(or_form, if cond { a } else { b });
                }
            }
        }
    }
}
