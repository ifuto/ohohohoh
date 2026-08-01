//! # Vanilla Minecraft UI Style Constants (`mc_style`)
//!
//! Mod が追加する UI を「マイクラ味」に保つための一次情報定数。
//! 値はバニラ 1.21.x の Screen/Button 実測規則に由来する:
//! - ボタン高 20px / 標準幅 200px (フル行)・98px (2 列グリッド)・150px (半幅)
//! - 行ピッチ 24px (タイトル/設定画面の標準スタック)
//! - テキスト: 白 0xFFFFFF / ホバー・推奨強調 黄 0xFFFF55・無効 灰 0xA0A0A0
//! - カテゴリ見出し: 金 0xFFAA00 (vanilla セクション見出し統一色)
//! - ツールチップ: 背景 0xF0100010 / 枠 0x505000FF (vanilla `Tooltip` 実色)
//!
//! 実消費者: `platform_bridge` (ホスト画面ボタン配置) と `cloth_config`
//! (設定行のバニラ風ラベル)。ここに無い ad-hoc なピクセル値を mod 側へ
//! 直書きしないための契約。

/// バニラ標準ボタン高 (全 Screen 共通)。
pub const BUTTON_H: i32 = 20;
/// フル幅ボタン (PauseScreen 中央列と同じ 200px)。
pub const BUTTON_W_FULL: i32 = 200;
/// 2 列グリッドの半幅ボタン (Options 画面 98px … 200 - 4*2 分割規則)。
pub const BUTTON_W_HALF: i32 = 98;
/// 中幅ボタン (Realms/言語系 150px)。
pub const BUTTON_W_MID: i32 = 150;
/// 縦スタックの標準ピッチ (button 20 + gap 4)。
pub const ROW_PITCH: i32 = 24;
/// ホスト画面の行間 (一覧行=ピッチ 22: 20 + 2 で一覧密度をバニラ ListWidget に寄せる)。
pub const LIST_ROW_PITCH: i32 = 22;

// ---- バニラテキスト/オーバーレイ色 (ARGB) ----
pub const TEXT_WHITE: u32 = 0xFFFFFFFF;
pub const TEXT_YELLOW: u32 = 0xFFFFFF55;
pub const TEXT_GOLD: u32 = 0xFFFFAA00;
pub const TEXT_GRAY: u32 = 0xFFA0A0A0;
pub const TEXT_RED: u32 = 0xFFFF5555;
/// 設定値が既定値から変わったときの強調色 (vanilla は黄色で注意喚起する習慣)。
pub const TEXT_MODIFIED: u32 = TEXT_YELLOW;

// ---- § 書式コード (クライアントの組込み装飾、リソースパック非依存) ----
pub const FMT_BOLD: &str = "§l";
pub const FMT_GOLD: &str = "§e";
pub const FMT_RESET: &str = "§r";

/// バニラ Screen タイトル行の装飾 (太字)。ホスト画面タイトルと同じ見え方に揃える。
pub fn title_text(title: &str) -> String {
    format!("{FMT_BOLD}{title}")
}

/// カテゴリ見出し行の装飾 (金色区切り。Mods 一覧のセクション見出しに使う)。
pub fn category_header(name: &str) -> String {
    format!("{FMT_GOLD}— {name} —")
}

/// 1 列レイアウトの縦位置系列 (y0 開始、ピッチ pitch)。mod のボタン配置を
/// 任意ピクセルではなく vanilla グリッドへ揃えるための状態機械。
#[derive(Debug, Clone)]
pub struct VanillaColumn {
    pub x: i32,
    pub w: i32,
    next_y: i32,
    pitch: i32,
}

impl VanillaColumn {
    /// full 幅 (200px) 列を y0 から。
    pub fn full(x: i32, y0: i32) -> Self {
        Self {
            x,
            w: BUTTON_W_FULL,
            next_y: y0,
            pitch: ROW_PITCH,
        }
    }

    pub fn with_pitch(mut self, pitch: i32) -> Self {
        self.pitch = pitch;
        self
    }

    /// 次の行の (x, y, w, h)。呼ぶたびに pitch 分だけ下がる。
    pub fn next_row(&mut self) -> (i32, i32, i32, i32) {
        let y = self.next_y;
        self.next_y += self.pitch;
        (self.x, y, self.w, BUTTON_H)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_geometry_pins() {
        // バニラ実測規約の退行防止ピン。
        assert_eq!(BUTTON_H, 20);
        assert_eq!(BUTTON_W_FULL, 200);
        assert_eq!(BUTTON_W_HALF, 98);
        assert_eq!(ROW_PITCH, 24);
    }

    #[test]
    fn text_decorations_match_vanilla() {
        assert_eq!(title_text("RsZoom Config"), "§lRsZoom Config");
        assert_eq!(category_header("Zoom"), "§e— Zoom —");
        assert_eq!(TEXT_MODIFIED, 0xFFFFFF55);
    }

    #[test]
    fn column_emits_stacked_vanilla_rows() {
        let mut col = VanillaColumn::full(20, 40).with_pitch(LIST_ROW_PITCH);
        assert_eq!(col.next_row(), (20, 40, 200, 20));
        assert_eq!(col.next_row(), (20, 62, 200, 20));
        assert_eq!(col.next_row(), (20, 84, 200, 20));
    }
}
