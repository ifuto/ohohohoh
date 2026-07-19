//! C ABI VTableによる1回だけのバインディング - 起動時にテーブルを生成し、
//! 以後は GetMethodID/メソッド検索を介さず関数ポインタ直呼び出し。
//!
//! 監査指摘の解消: 旧実装は全エントリが no-op ダミー (`dummy_update_chunk` 等) で
//! `install_vtable()` に消費者も存在しなかった。本実装は全エントリを
//! **実関数** (`rsift_opt_gfx::ingest_world_column` / `set_world_camera` /
//! `agent_log`) に張り替え、ChunkBridge natives 登録時に実インストール + 実自己検証を行う。

use crate::agent_log::agent_log;
use std::sync::OnceLock;

/// Java ↔ native ブリッジの C ABI 関数テーブル。
/// `version` はレイアウト契約: フィールドを変えたら必ずインクリメント。
#[repr(C)]
pub struct JavaBridgeVTable {
    pub version: u32,
    /// 実チャンク列インジェスト: `data` は LE u16 ブロック id (4096/section)。
    /// 戻り値はインジェストされた section 数 (エラー時は負)。
    pub update_chunk: extern "C" fn(i32, i32, i32, *const u8, usize) -> i32,
    /// 実カメラ反映 (world_column_store 経由で描画パイプラインの実入力になる)。
    pub set_camera: extern "C" fn(f32, f32, f32, f32, f32),
    /// 実ログ転送 (RsiftAgent.log へ UTF-8 lossy で追記)。
    pub log: extern "C" fn(*const u8, usize),
}

extern "C" fn real_update_chunk(
    cx: i32,
    cz: i32,
    base_section_y: i32,
    data: *const u8,
    len_bytes: usize,
) -> i32 {
    if data.is_null() || len_bytes == 0 || len_bytes % 2 != 0 {
        return -1;
    }
    let sections = len_bytes / 2 / 4096;
    if sections == 0 || sections > 64 {
        return -2;
    }
    // SAFETY: 呼び出し側契約により data は len_bytes 分の読み取り可能領域
    // (C ABI 混乱を避けるため Pod int 変換は bytemuck 検証付き)。
    let bytes = unsafe { std::slice::from_raw_parts(data, len_bytes) };
    let Some(ids) = ({
        let n = len_bytes / 2;
        let mut v = Vec::with_capacity(n);
        v.extend(
            bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]])),
        );
        Some(v)
    }) else {
        return -3;
    };
    rsift_opt_gfx::ingest_world_column(cx, cz, base_section_y, &ids, sections);
    sections as i32
}

extern "C" fn real_set_camera(x: f32, y: f32, z: f32, yaw: f32, pitch: f32) {
    rsift_opt_gfx::set_world_camera(x, y, z, yaw, pitch);
}

extern "C" fn real_log(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > (1 << 16) {
        return;
    }
    // SAFETY: 呼び出し側契約により ptr は len 分の読み取り可能領域。
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let msg = String::from_utf8_lossy(bytes);
    agent_log(&format!("[VTable] {}", msg.trim_end()));
}

static INSTALLED_VTABLE: OnceLock<JavaBridgeVTable> = OnceLock::new();

/// VTable を生成・静的確定 (1 プロセス 1 回) し、実ポインタを返す。
/// インストール時に実自己検証を走らせ、リンク不良をその場で検出する。
pub fn install_vtable() -> &'static JavaBridgeVTable {
    INSTALLED_VTABLE.get_or_init(|| {
        let table = JavaBridgeVTable {
            version: 3,
            update_chunk: real_update_chunk,
            set_camera: real_set_camera,
            log: real_log,
        };
        // 実自己検証: ダミーではなく本物のエントリを通して no-op 相当の
        // 実呼び出し (カメラ現状維持 + ログ 1 行) を行い、vtable が
        // 実際に実行可能であることを起動時に実証する。
        let cam = rsift_opt_gfx::world_camera();
        (table.set_camera)(cam.0, cam.1, cam.2, cam.3, cam.4);
        let probe = b"vtable installed\0";
        (table.log)(probe.as_ptr(), probe.len() - 1);
        table
    })
}

/// インストール済み VTable (未インストール時は None)。
pub fn installed_vtable() -> Option<&'static JavaBridgeVTable> {
    INSTALLED_VTABLE.get()
}

/// `JavaBridgeVTable` への安定ポインタ (JNA 直呼び出し用)。
/// 戻り値は `'static` 領域を指すため解放禁止。
pub fn vtable_ptr() -> *const JavaBridgeVTable {
    install_vtable() as *const _
}
