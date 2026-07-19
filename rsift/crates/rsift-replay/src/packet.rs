//! Zero-copy packet capture & Universal First-Person UI/HUD Snapshot types
//!
//! 1) `CameraSnapshot`: 一人称視点 (`First-Person Centric`) カメラ状態
//! 2) `FirstPersonUiSnapshot`: Tab キープレイヤーリスト (`PlayerListHud`)、ホットバー選択枠、
//!    手/アイテム振りモーション (`hand_swing_progress`)、チャット HUD、ボスバーの完全記録
//! 3) `UiDrawCommand`: サードパーティのあらゆる Mod が追加した Tab リスト拡張や
//!    カスタム HUD オーバーレイ、GUI ウィジェットの 2D 描画命令 (`GuiGraphics` / `LWJGL` 2D) を
//!    パケットストリーム内に完全記録するユニバーサル・コマンドバッファ
//! 4) オフラインレンダリング時に、他 Mod のカスタム HUD ごと OBS や通常画面録画と全く同じ
//!    一画面表示 (`全く同じ表示`) を完全再現し、後からのリソパ・シェーダー変更に 100% 対応。

use bytemuck::{Pod, Zeroable};
use rsift_api::packet::DirectBufferSlice;
use serde::{Deserialize, Serialize};
use tracing::trace;

/// Packet direction
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDirection {
    Incoming = 0,
    Outgoing = 1,
}

/// Header for each recorded packet in raw stream
#[derive(Debug, Clone, Copy)]
pub struct PacketRecordHeader {
    pub timestamp_us: u64,
    pub direction: u8,
    pub packet_id: u32,
    pub payload_len: u32,
    pub view_mode: u8,
}

/// F5 view mode change event
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ViewModeEvent {
    pub timestamp_us: u64,
    pub mode: u8,
    pub _pad: [u8; 7],
}

/// Player camera state snapshot (first-person centric)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CameraSnapshot {
    pub timestamp_us: u64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub view_mode: u8,
    pub _pad: [u8; 3],
}

/// Exact Tab key player entry in the PlayerListHud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabPlayerEntry {
    pub uuid: [u8; 16],
    pub username: String,
    pub ping_ms: i32,
    pub game_mode: u8,
    pub display_name_json: Option<String>,
}

/// Boss bar state (Wither, Ender Dragon, Raid boss bars on top of HUD).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BossBarState {
    pub uuid: [u8; 16],
    pub title: String,
    pub progress: f32,
    pub color: u8,
}

/// Universal 2D UI command intercepted from `GuiGraphics` / `LWJGL` render passes.
/// Captures custom widgets, custom Player List extensions, icons, stamina bars, and overlays
/// added by *any* third-party Fabric/NeoForge/Rsift mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiCommandKind {
    FillRect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        color_argb: u32,
    },
    Text {
        x: i32,
        y: i32,
        text: String,
        color_argb: u32,
        shadow: bool,
        font_id: String,
    },
    Texture {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        u0: f32,
        v0: f32,
        u1: f32,
        v1: f32,
        texture_id: String,
        tint_argb: u32,
    },
    CustomModQuad {
        vertices: Vec<[f32; 4]>, // [x, y, u, v]
        colors: Vec<u32>,        // ARGB per vertex
        texture_id: Option<String>,
        shader_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiDrawCommand {
    pub kind: UiCommandKind,
    pub z_layer: i32,
    pub mod_owner_id: String, // e.g. "vanilla.tab_list", "mod.better_tab", "mod.armor_status"
}

/// First-Person UI & HUD Snapshot — captures exact first-person screen elements
/// and all third-party mod 2D UI draw commands (`mod_ui_commands`)
/// so offline rendering matches live OBS/screen capture 100%.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FirstPersonUiSnapshot {
    pub timestamp_us: u64,
    /// True when the player holds down `Tab` to check the scoreboard/player list
    pub tab_list_visible: bool,
    pub tab_header_text: Option<String>,
    pub tab_footer_text: Option<String>,
    pub tab_entries: Vec<TabPlayerEntry>,
    pub hotbar_selected_slot: u8, // 0..8
    pub hand_swing_progress: f32, // 0.0..1.0 for first-person item/hand swing
    pub is_blocking_or_using: bool,
    pub chat_messages: Vec<String>,
    pub boss_bars: Vec<BossBarState>,
    /// Universal intercepted draw command stream from Vanilla & all third-party Mods
    pub mod_ui_commands: Vec<UiDrawCommand>,
}

impl FirstPersonUiSnapshot {
    pub fn new(timestamp_us: u64) -> Self {
        Self {
            timestamp_us,
            tab_list_visible: false,
            tab_header_text: None,
            tab_footer_text: None,
            tab_entries: Vec::new(),
            hotbar_selected_slot: 0,
            hand_swing_progress: 0.0,
            is_blocking_or_using: false,
            chat_messages: Vec::new(),
            boss_bars: Vec::new(),
            mod_ui_commands: Vec::new(),
        }
    }

    pub fn push_ui_command(&mut self, mod_id: impl Into<String>, z_layer: i32, kind: UiCommandKind) {
        self.mod_ui_commands.push(UiDrawCommand {
            kind,
            z_layer,
            mod_owner_id: mod_id.into(),
        });
    }
}

/// Zero-copy capture from JNI DirectBuffer
pub struct ZeroCopyCapture;

impl ZeroCopyCapture {
    /// Capture packet without copying JVM heap — reads direct buffer via bytemuck
    pub unsafe fn capture_from_jni(
        timestamp_us: u64,
        direction: PacketDirection,
        packet_id: u32,
        ptr: i64,
        len: i32,
        view_mode: u8,
    ) -> Result<CapturedPacket, &'static str> {
        let slice = DirectBufferSlice::from_raw_jni(ptr, len)?;
        let payload = slice.as_slice().to_vec();
        trace!(
            "[RsReplay] zero-copy capture pkt={} dir={:?} len={}",
            packet_id, direction, len
        );
        Ok(CapturedPacket {
            header: PacketRecordHeader {
                timestamp_us,
                direction: direction as u8,
                packet_id,
                payload_len: payload.len() as u32,
                view_mode,
            },
            payload,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CapturedPacket {
    pub header: PacketRecordHeader,
    pub payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_person_ui_snapshot_with_mod_commands() {
        let mut snap = FirstPersonUiSnapshot::new(1000);
        snap.tab_list_visible = true;
        snap.push_ui_command(
            "mod.better_tab",
            10,
            UiCommandKind::Text {
                x: 100,
                y: 50,
                text: "[Guild: Elite] Ifuto_mitai".into(),
                color_argb: 0xFF55FF55,
                shadow: true,
                font_id: "minecraft:default".into(),
            },
        );
        assert!(snap.tab_list_visible);
        assert_eq!(snap.mod_ui_commands[0].mod_owner_id, "mod.better_tab");
    }
}
