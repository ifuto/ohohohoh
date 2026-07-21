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
        // 分岐無し: テーブル参照のみ
        self.opaque[id as usize & 4095]
    }

    #[inline(always)]
    pub fn light_branchless(&self, id: u16) -> u8 {
        self.light[id as usize & 4095]
    }

    #[inline(always)]
    pub fn select_branchless(cond: bool, a: u32, b: u32) -> u32 {
        // cmov相当: (cond as u32 * a) + (!cond as u32 * b)を算術で
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
}
