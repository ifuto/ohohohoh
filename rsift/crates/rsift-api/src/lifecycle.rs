//! # Rsift Lifecycle & Event System
//!
//! Full Fabric-parity lifecycle bus. JVM `RsiftPlatformBridge` fires these
//! callbacks from Minecraft tick / connection / world hooks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, debug};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentType {
    Client,
    DedicatedServer,
}

pub struct ModLoaderEnvironment {
    pub game_dir: PathBuf,
    pub config_dir: PathBuf,
    pub mods_dir: PathBuf,
    pub environment_type: EnvironmentType,
    pub loaded_mod_ids: std::collections::HashSet<String>,
}

impl ModLoaderEnvironment {
    pub fn new(game_dir: PathBuf, environment_type: EnvironmentType) -> Self {
        let config_dir = game_dir.join("config");
        let mods_dir = game_dir.join("mods");
        Self {
            game_dir,
            config_dir,
            mods_dir,
            environment_type,
            loaded_mod_ids: std::collections::HashSet::new(),
        }
    }

    pub fn is_mod_loaded(&self, mod_id: &str) -> bool {
        self.loaded_mod_ids.contains(mod_id)
    }

    pub fn get_config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn get_game_dir(&self) -> &Path {
        &self.game_dir
    }

    pub fn mark_mod_loaded(&mut self, mod_id: impl Into<String>) {
        self.loaded_mod_ids.insert(mod_id.into());
    }
}

pub trait ModInitializer: Send + Sync {
    fn on_init(&self, env: &ModLoaderEnvironment) -> Result<(), String>;
}

pub trait ClientModInitializer: Send + Sync {
    fn on_client_init(&self, env: &ModLoaderEnvironment) -> Result<(), String>;
}

pub trait DedicatedServerModInitializer: Send + Sync {
    fn on_server_init(&self, env: &ModLoaderEnvironment) -> Result<(), String>;
}

pub type ServerStartingFn = Arc<dyn Fn() + Send + Sync>;
pub type ServerStartedFn = Arc<dyn Fn() + Send + Sync>;
pub type ServerStoppingFn = Arc<dyn Fn() + Send + Sync>;
pub type ServerStoppedFn = Arc<dyn Fn() + Send + Sync>;

pub type ServerTickStartFn = Arc<dyn Fn(u64) + Send + Sync>;
pub type ServerTickEndFn = Arc<dyn Fn(u64) + Send + Sync>;
pub type WorldTickStartFn = Arc<dyn Fn(&str, u64) + Send + Sync>;
pub type WorldTickEndFn = Arc<dyn Fn(&str, u64) + Send + Sync>;

pub type ClientStartedFn = Arc<dyn Fn() + Send + Sync>;
pub type ClientStoppingFn = Arc<dyn Fn() + Send + Sync>;
pub type ClientTickStartFn = Arc<dyn Fn(u64) + Send + Sync>;
pub type ClientTickEndFn = Arc<dyn Fn(u64) + Send + Sync>;

pub type WorldLoadFn = Arc<dyn Fn(&str) + Send + Sync>;
pub type WorldUnloadFn = Arc<dyn Fn(&str) + Send + Sync>;
pub type ChunkLoadFn = Arc<dyn Fn(&str, i32, i32) + Send + Sync>;
pub type ChunkUnloadFn = Arc<dyn Fn(&str, i32, i32) + Send + Sync>;
pub type ChunkWatchFn = Arc<dyn Fn(&str, i32, i32, &str) + Send + Sync>;

pub type PlayerJoinFn = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type PlayerDisconnectFn = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type PlayerRespawnFn = Arc<dyn Fn(&str, bool) + Send + Sync>;
pub type EntityLoadFn = Arc<dyn Fn(u32, &str, f64, f64, f64) + Send + Sync>;
pub type EntityUnloadFn = Arc<dyn Fn(u32, &str) + Send + Sync>;

#[derive(Default, Clone)]
pub struct EventBus {
    pub server_starting_callbacks: Vec<ServerStartingFn>,
    pub server_started_callbacks: Vec<ServerStartedFn>,
    pub server_stopping_callbacks: Vec<ServerStoppingFn>,
    pub server_stopped_callbacks: Vec<ServerStoppedFn>,
    pub server_tick_start_callbacks: Vec<ServerTickStartFn>,
    pub server_tick_end_callbacks: Vec<ServerTickEndFn>,
    pub world_tick_start_callbacks: Vec<WorldTickStartFn>,
    pub world_tick_end_callbacks: Vec<WorldTickEndFn>,
    pub client_started_callbacks: Vec<ClientStartedFn>,
    pub client_stopping_callbacks: Vec<ClientStoppingFn>,
    pub client_tick_start_callbacks: Vec<ClientTickStartFn>,
    pub client_tick_end_callbacks: Vec<ClientTickEndFn>,
    pub world_load_callbacks: Vec<WorldLoadFn>,
    pub world_unload_callbacks: Vec<WorldUnloadFn>,
    pub chunk_load_callbacks: Vec<ChunkLoadFn>,
    pub chunk_unload_callbacks: Vec<ChunkUnloadFn>,
    pub chunk_watch_callbacks: Vec<ChunkWatchFn>,
    pub player_join_callbacks: Vec<PlayerJoinFn>,
    pub player_disconnect_callbacks: Vec<PlayerDisconnectFn>,
    pub player_respawn_callbacks: Vec<PlayerRespawnFn>,
    pub entity_load_callbacks: Vec<EntityLoadFn>,
    pub entity_unload_callbacks: Vec<EntityUnloadFn>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_server_starting(&mut self, cb: ServerStartingFn) {
        debug!("Registering ServerStarting lifecycle callback");
        self.server_starting_callbacks.push(cb);
    }

    pub fn register_server_started(&mut self, cb: ServerStartedFn) {
        self.server_started_callbacks.push(cb);
    }

    pub fn register_server_stopping(&mut self, cb: ServerStoppingFn) {
        self.server_stopping_callbacks.push(cb);
    }

    pub fn register_server_stopped(&mut self, cb: ServerStoppedFn) {
        self.server_stopped_callbacks.push(cb);
    }

    pub fn register_server_tick_start(&mut self, cb: ServerTickStartFn) {
        self.server_tick_start_callbacks.push(cb);
    }

    pub fn register_server_tick_end(&mut self, cb: ServerTickEndFn) {
        self.server_tick_end_callbacks.push(cb);
    }

    pub fn register_client_started(&mut self, cb: ClientStartedFn) {
        self.client_started_callbacks.push(cb);
    }

    pub fn register_client_stopping(&mut self, cb: ClientStoppingFn) {
        self.client_stopping_callbacks.push(cb);
    }

    pub fn register_client_tick_start(&mut self, cb: ClientTickStartFn) {
        self.client_tick_start_callbacks.push(cb);
    }

    pub fn register_client_tick_end(&mut self, cb: ClientTickEndFn) {
        self.client_tick_end_callbacks.push(cb);
    }

    pub fn register_world_load(&mut self, cb: WorldLoadFn) {
        self.world_load_callbacks.push(cb);
    }

    pub fn register_world_unload(&mut self, cb: WorldUnloadFn) {
        self.world_unload_callbacks.push(cb);
    }

    pub fn register_chunk_load(&mut self, cb: ChunkLoadFn) {
        self.chunk_load_callbacks.push(cb);
    }

    pub fn register_chunk_unload(&mut self, cb: ChunkUnloadFn) {
        self.chunk_unload_callbacks.push(cb);
    }

    pub fn register_chunk_watch(&mut self, cb: ChunkWatchFn) {
        self.chunk_watch_callbacks.push(cb);
    }

    pub fn register_player_join(&mut self, cb: PlayerJoinFn) {
        self.player_join_callbacks.push(cb);
    }

    pub fn register_player_disconnect(&mut self, cb: PlayerDisconnectFn) {
        self.player_disconnect_callbacks.push(cb);
    }

    pub fn register_player_respawn(&mut self, cb: PlayerRespawnFn) {
        self.player_respawn_callbacks.push(cb);
    }

    pub fn register_entity_load(&mut self, cb: EntityLoadFn) {
        self.entity_load_callbacks.push(cb);
    }

    pub fn register_entity_unload(&mut self, cb: EntityUnloadFn) {
        self.entity_unload_callbacks.push(cb);
    }

    pub fn dispatch_server_starting(&self) {
        for cb in &self.server_starting_callbacks {
            cb();
        }
    }

    pub fn dispatch_server_started(&self) {
        for cb in &self.server_started_callbacks {
            cb();
        }
    }

    pub fn dispatch_server_stopping(&self) {
        for cb in &self.server_stopping_callbacks {
            cb();
        }
    }

    pub fn dispatch_server_stopped(&self) {
        for cb in &self.server_stopped_callbacks {
            cb();
        }
    }

    pub fn dispatch_server_tick(&self, tick: u64) {
        for cb in &self.server_tick_start_callbacks {
            cb(tick);
        }
        for cb in &self.server_tick_end_callbacks {
            cb(tick);
        }
    }

    pub fn dispatch_client_started(&self) {
        for cb in &self.client_started_callbacks {
            cb();
        }
    }

    pub fn dispatch_client_stopping(&self) {
        for cb in &self.client_stopping_callbacks {
            cb();
        }
    }

    pub fn dispatch_client_tick(&self) {
        for cb in &self.client_tick_start_callbacks {
            cb(0);
        }
        for cb in &self.client_tick_end_callbacks {
            cb(0);
        }
    }

    pub fn dispatch_client_tick_numbered(&self, tick: u64) {
        for cb in &self.client_tick_start_callbacks {
            cb(tick);
        }
        for cb in &self.client_tick_end_callbacks {
            cb(tick);
        }
    }

    pub fn dispatch_world_load(&self, dim: &str) {
        for cb in &self.world_load_callbacks {
            cb(dim);
        }
    }

    pub fn dispatch_world_unload(&self, dim: &str) {
        for cb in &self.world_unload_callbacks {
            cb(dim);
        }
    }

    pub fn dispatch_chunk_load(&self, dim: &str, cx: i32, cz: i32) {
        for cb in &self.chunk_load_callbacks {
            cb(dim, cx, cz);
        }
    }

    pub fn dispatch_chunk_unload(&self, dim: &str, cx: i32, cz: i32) {
        for cb in &self.chunk_unload_callbacks {
            cb(dim, cx, cz);
        }
    }

    pub fn dispatch_chunk_watch(&self, dim: &str, cx: i32, cz: i32, player: &str) {
        for cb in &self.chunk_watch_callbacks {
            cb(dim, cx, cz, player);
        }
    }

    pub fn dispatch_player_join(&self, name: &str, uuid: &str) {
        for cb in &self.player_join_callbacks {
            cb(name, uuid);
        }
    }

    pub fn dispatch_player_disconnect(&self, name: &str, uuid: &str) {
        for cb in &self.player_disconnect_callbacks {
            cb(name, uuid);
        }
    }

    pub fn dispatch_player_respawn(&self, name: &str, conquered: bool) {
        for cb in &self.player_respawn_callbacks {
            cb(name, conquered);
        }
    }

    pub fn dispatch_entity_load(&self, id: u32, ty: &str, x: f64, y: f64, z: f64) {
        for cb in &self.entity_load_callbacks {
            cb(id, ty, x, y, z);
        }
    }

    pub fn dispatch_entity_unload(&self, id: u32, ty: &str) {
        for cb in &self.entity_unload_callbacks {
            cb(id, ty);
        }
    }

    /// Dispatch a named lifecycle event from JVM (string op).
    pub fn dispatch_named(&self, op: &str, a: &str, b: &str, n0: i64, n1: i64, _n2: i64) {
        match op {
            "server_starting" => self.dispatch_server_starting(),
            "server_started" => self.dispatch_server_started(),
            "server_stopping" => self.dispatch_server_stopping(),
            "server_stopped" => self.dispatch_server_stopped(),
            "server_tick" => self.dispatch_server_tick(n0 as u64),
            "client_started" => self.dispatch_client_started(),
            "client_stopping" => self.dispatch_client_stopping(),
            "client_tick" => self.dispatch_client_tick_numbered(n0 as u64),
            "world_load" => self.dispatch_world_load(a),
            "world_unload" => self.dispatch_world_unload(a),
            "chunk_load" => self.dispatch_chunk_load(a, n0 as i32, n1 as i32),
            "chunk_unload" => self.dispatch_chunk_unload(a, n0 as i32, n1 as i32),
            "chunk_watch" => self.dispatch_chunk_watch(a, n0 as i32, n1 as i32, b),
            "player_join" => self.dispatch_player_join(a, b),
            "player_disconnect" => self.dispatch_player_disconnect(a, b),
            "player_respawn" => self.dispatch_player_respawn(a, n0 != 0),
            "entity_load" => {
                // a=type, n0=id, n1/n2 unused for coords (passed as packed in b "x,y,z")
                let (x, y, z) = parse_xyz(b);
                self.dispatch_entity_load(n0 as u32, a, x, y, z);
            }
            "entity_unload" => self.dispatch_entity_unload(n0 as u32, a),
            other => debug!("[EventBus] unknown lifecycle op: {}", other),
        }
    }
}

fn parse_xyz(s: &str) -> (f64, f64, f64) {
    let mut parts = s.split(',');
    let x = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0.0);
    let y = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0.0);
    let z = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0.0);
    (x, y, z)
}

/// Gametest hooks placeholder used by fabric module catalog binding.
pub struct GametestHooks;

impl GametestHooks {
    pub fn register(name: &str, fn_symbol: &str) {
        tracing::warn!(
            "[GametestHooks] catalog bind only — no test runner (name={name} symbol={fn_symbol}). \
             Not claiming gametest execution."
        );
        crate::platform::mark_dirty();
    }
}
