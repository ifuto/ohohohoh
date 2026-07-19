//! Packet filtering — drop packets irrelevant to first-person replay

/// Packets to exclude from recording (keepalive, remote inventory sync, etc.)
pub struct PacketFilter {
    pub first_person_only: bool,
    pub fov_cull_radius: f32,
}

impl Default for PacketFilter {
    fn default() -> Self {
        Self {
            first_person_only: true,
            fov_cull_radius: 128.0,
        }
    }
}

impl PacketFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if packet should be recorded
    pub fn should_record(&self, packet_id: u32, direction: u8) -> bool {
        // Drop keepalive / ping / unrelated sync
        if Self::is_dropped_id(packet_id) {
            return false;
        }
        // Always record outgoing player actions (movement, chat, interact)
        if direction == 1 {
            return true;
        }
        // Incoming: record world state, entities in view, GUI, chat, TAB
        !Self::is_remote_inventory_sync(packet_id)
    }

    fn is_dropped_id(id: u32) -> bool {
        matches!(id,
            0x00 | 0x01 | 0x0F | 0x10 | 0x11 | 0x12 | // keepalive, ping, chunk cache
            0x33 | 0x34  // light updates far from player (simplified)
        )
    }

    fn is_remote_inventory_sync(id: u32) -> bool {
        matches!(id, 0x14 | 0x15 | 0x16) // other player inventory windows
    }

    /// TAB scoreboard, chat, GUI packets — always record
    pub fn is_ui_packet(packet_id: u32) -> bool {
        matches!(packet_id,
            0x0E | 0x0F | // chat
            0x42 | 0x43 | // tab list, scoreboard
            0x2B | 0x2C   // open screen, set slot (GUI)
        )
    }
}
