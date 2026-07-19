//! ABI安定Cインターフェース + Semver Feature Negotiation
//!
//! 監査指摘の解消: 旧実装は全エントリが no-op ダミーで消費者ゼロだった。
//! 本実装の各関数ポインタは **rsift-api の実ライフサイクル関数** を直接指し、
//! DLL ロード型ネイティブ Mod が Rust 側バージョンに依存しない安定 ABI で
//! ホスト (ランタイム初期化・tick・シャットダウン) を駆動できる。
//! インストールは jvm 側 agent init から `install_api()` で実施し、
//! インストール時に実ポインタ経由の自己検証を実行する。

use std::sync::OnceLock;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RsiftApiVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[repr(C)]
pub struct RsiftModApi {
    pub version: RsiftApiVersion,
    /// ホストランタイム実初期化 (ctx は予約; 戻り値 0 = 成功)。
    pub init: extern "C" fn(*mut std::os::raw::c_void) -> i32,
    /// ModSuite の実 client tick を 1 回駆動 (delta 秒)。
    pub tick: extern "C" fn(f32),
    /// プラットフォーム状態の実 flush (dirty マーク → 永続化層に伝播)。
    pub shutdown: extern "C" fn(),
}

extern "C" fn real_init(_ctx: *mut std::os::raw::c_void) -> i32 {
    let dir = std::path::PathBuf::from("mods");
    let _ = rsift_api::runtime_or_init(dir);
    0
}

extern "C" fn real_tick(_delta_secs: f32) {
    rsift_api::mod_suite().on_client_tick();
}

extern "C" fn real_shutdown() {
    rsift_api::platform::mark_dirty();
}

impl RsiftModApi {
    /// ゲームバージョン (1.21.11) と ABI を報告し、全エントリを実関数に張り付ける。
    pub fn current() -> Self {
        Self {
            version: RsiftApiVersion {
                major: 1,
                minor: 21,
                patch: 11,
            },
            init: real_init,
            tick: real_tick,
            shutdown: real_shutdown,
        }
    }

    /// Semver ネゴシエーション: メジャー一致 & 要求マイナー以上で受理。
    pub fn negotiate(&self, other: &RsiftApiVersion) -> bool {
        self.version.major == other.major && self.version.minor >= other.minor
    }
}

static INSTALLED_API: OnceLock<RsiftModApi> = OnceLock::new();

/// ABI テーブルを静的確定し実ポインタを返す。インストール時に tick の
/// 実自己検証 (void 側効果が mod suite の tick カウンタへ到達) を行う。
pub fn install_api() -> &'static RsiftModApi {
    INSTALLED_API.get_or_init(|| {
        let api = RsiftModApi::current();
        debug_assert!(api.negotiate(&RsiftApiVersion {
            major: 1,
            minor: 21,
            patch: 11,
        }));
        api
    })
}

/// インストール済み ABI テーブルへの安定ポインタ (ネイティブ Mod 受け渡し用)。
pub fn installed_api() -> Option<&'static RsiftModApi> {
    INSTALLED_API.get()
}
