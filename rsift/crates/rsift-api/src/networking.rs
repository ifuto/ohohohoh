//! # Rsift Networking System (Fabric Networking API Parity & Zero-Copy Integration)
//!
//! Fabric の `ServerPlayNetworking`, `ClientPlayNetworking`, `ServerLoginNetworking`,
//! `ClientLoginNetworking` の全インターフェースを包含し、Rsift の特徴である
//! Netty Direct ByteBuffer & `bytemuck` ゼロコピーポインタブリッジに統合します。

use crate::packet::DirectBufferSlice;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// ネットワークチャネルの識別子 (例: `rsift:custom_payload`)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelId {
    pub namespace: String,
    pub path: String,
}

impl ChannelId {
    pub fn new(namespace: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            path: path.into(),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{}:{}", self.namespace, self.path)
    }
}

/// パケット送信ヘルパートレイト (`PacketSender`)
pub trait PacketSender: Send + Sync {
    /// クライアントまたはサーバーへゼロコピーでパケットデータを送信する
    fn send_packet(&self, channel: &ChannelId, raw_ptr: i64, len: i32) -> Result<(), String>;
}

/// ゼロコピーパケット受信ハンドラトレイト (`PlayChannelHandler` / `LoginChannelHandler`)
pub trait PlayChannelHandler: Send + Sync {
    /// パケットを受信した際にオフヒープメモリポインタとともに呼び出される
    fn receive(
        &self,
        channel: &ChannelId,
        slice: &DirectBufferSlice,
        sender: &dyn PacketSender,
    ) -> bool;
}

/// 統合ネットワークマネージャー (`ServerPlayNetworking` / `ClientPlayNetworking`)
#[derive(Default)]
pub struct NetworkManager {
    pub server_play_handlers: HashMap<ChannelId, Arc<dyn PlayChannelHandler>>,
    pub client_play_handlers: HashMap<ChannelId, Arc<dyn PlayChannelHandler>>,
    pub login_handlers: HashMap<ChannelId, Arc<dyn PlayChannelHandler>>,
    pub global_receivers: Vec<Arc<dyn PlayChannelHandler>>,
}

impl NetworkManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// サーバー側プレイパケット受信チャネルを登録 (`ServerPlayNetworking.registerGlobalReceiver`)
    pub fn register_server_receiver(
        &mut self,
        channel: ChannelId,
        handler: Arc<dyn PlayChannelHandler>,
    ) {
        info!(
            "Registering ServerPlayNetworking receiver for channel [{}]",
            channel.as_str()
        );
        self.server_play_handlers.insert(channel, handler);
        crate::platform::mark_dirty();
    }

    /// クライアント側プレイパケット受信チャネルを登録 (`ClientPlayNetworking.registerGlobalReceiver`)
    pub fn register_client_receiver(
        &mut self,
        channel: ChannelId,
        handler: Arc<dyn PlayChannelHandler>,
    ) {
        info!(
            "Registering ClientPlayNetworking receiver for channel [{}]",
            channel.as_str()
        );
        self.client_play_handlers.insert(channel, handler);
        crate::platform::mark_dirty();
    }

    /// ログイン・ハンドシェイクチャネルを登録 (`ServerLoginNetworking` / `ClientLoginNetworking`)
    pub fn register_login_receiver(
        &mut self,
        channel: ChannelId,
        handler: Arc<dyn PlayChannelHandler>,
    ) {
        info!(
            "Registering LoginNetworking receiver for channel [{}]",
            channel.as_str()
        );
        self.login_handlers.insert(channel, handler);
        crate::platform::mark_dirty();
    }

    /// Nettyの直接メモリから渡された生ポインタを解決してハンドラへディスパッチする
    pub fn dispatch_raw(
        &self,
        channel_str: &str,
        ptr: i64,
        len: i32,
        is_client: bool,
        sender: &dyn PacketSender,
    ) -> bool {
        let parts: Vec<&str> = channel_str.split(':').collect();
        let channel = if parts.len() == 2 {
            ChannelId::new(parts[0], parts[1])
        } else {
            ChannelId::new("minecraft", channel_str)
        };

        let slice = match unsafe { DirectBufferSlice::from_raw_jni(ptr, len) } {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "DirectBuffer slice error on channel [{}]: {}",
                    channel_str, e
                );
                return true;
            }
        };

        let handlers = if is_client {
            &self.client_play_handlers
        } else {
            &self.server_play_handlers
        };

        if let Some(handler) = handlers.get(&channel) {
            debug!(
                "Dispatching zero-copy packet on channel [{}] (size: {} bytes)",
                channel_str, len
            );
            return handler.receive(&channel, &slice, sender);
        }

        for handler in &self.global_receivers {
            if !handler.receive(&channel, &slice, sender) {
                return false;
            }
        }
        true
    }

    pub fn register_global_receiver(&mut self, handler: Arc<dyn PlayChannelHandler>) {
        self.global_receivers.push(handler);
        crate::platform::mark_dirty();
    }
}

/// JNI-backed packet sender — queues payloads for the platform bridge to transmit.
#[derive(Default)]
pub struct PlatformPacketSender;

impl PacketSender for PlatformPacketSender {
    fn send_packet(&self, channel: &ChannelId, raw_ptr: i64, len: i32) -> Result<(), String> {
        if raw_ptr == 0 || len <= 0 {
            return Err("invalid buffer".into());
        }
        let slice =
            unsafe { DirectBufferSlice::from_raw_jni(raw_ptr, len) }.map_err(|e| e.to_string())?;
        let bytes = slice.as_slice().to_vec();
        crate::platform::enqueue_outbound_payload(&channel.as_str(), bytes);
        Ok(())
    }
}

impl PlatformPacketSender {
    pub fn send_bytes(&self, channel: &ChannelId, bytes: &[u8]) {
        crate::platform::enqueue_outbound_payload(&channel.as_str(), bytes.to_vec());
    }
}
