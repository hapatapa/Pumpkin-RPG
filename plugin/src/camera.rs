//! Camera system — pitch/yaw-based camera modes.
//!
//! The original design used CSetCamera to spectate invisible armor stands
//! (the "fake player" approach). This doesn't work for gameplay because:
//!   - CSetCamera is Minecraft's SPECTATE packet — when spectating, the
//!     client stops sending movement input (WASD does nothing)
//!   - The spectated armor stand can't rotate from mouse input, so the
//!     view is frozen (unrotable)
//!   - The player can't move their own character
//!
//! Minecraft has NO server-side packet for a gameplay third-person camera.
//! Real third-person cameras (like F5) are client-side only.
//!
//! This rewrite uses the only server-side camera control available: setting
//! the player's pitch/yaw. Camera modes force specific pitch angles:
//!   - FirstPerson: no change (player controls freely)
//!   - OverShoulder: slight downward tilt (10°) — use F5 for third-person view
//!   - TopDown: forces pitch to 80° (looking down) for isometric feel
//!   - Cinematic: no forced angle (player controls freely; future: FOV changes)
//!   - CombatCam: slight downward tilt (15°) for combat awareness
//!
//! Players can still rotate freely in all modes except TopDown (which
//! re-centers pitch every tick). This allows full movement + rotation.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::LazyLock;
use std::collections::HashMap;
use std::sync::Mutex;

use pumpkin::server::Server;

/// Camera modes. Selected via `/camera <mode>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraMode {
    FirstPerson,
    OverShoulder,
    TopDown,
    Cinematic,
    CombatCam,
}

impl CameraMode {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "firstperson" | "first_person" | "fp" | "default" => Some(Self::FirstPerson),
            "overshoulder" | "over_shoulder" | "os" | "thirdperson" | "third" => Some(Self::OverShoulder),
            "topdown" | "top_down" | "td" | "top" => Some(Self::TopDown),
            "cinematic" | "cin" => Some(Self::Cinematic),
            "combatcam" | "combat_cam" | "cc" | "combat" => Some(Self::CombatCam),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FirstPerson => "First Person",
            Self::OverShoulder => "Over Shoulder",
            Self::TopDown => "Top Down",
            Self::Cinematic => "Cinematic",
            Self::CombatCam => "Combat Cam",
        }
    }

    /// The pitch angle (in degrees) to force for this mode.
    /// Returns None if the player should control pitch freely.
    /// Positive pitch = looking down in Minecraft.
    pub fn forced_pitch(&self) -> Option<f32> {
        match self {
            Self::FirstPerson => None,
            Self::OverShoulder => Some(10.0),   // slight downward tilt
            Self::TopDown => Some(80.0),         // nearly straight down
            Self::Cinematic => None,             // free look
            Self::CombatCam => Some(15.0),       // slight downward for combat awareness
        }
    }

    /// Description shown to the player when they switch modes.
    pub fn description(&self) -> &'static str {
        match self {
            Self::FirstPerson => "Default first-person view. Full movement and rotation.",
            Self::OverShoulder => "Slight downward tilt. Press F5 in-game for third-person view.",
            Self::TopDown => "Forces pitch to 80 (looking down). Isometric-style. You can still rotate horizontally.",
            Self::Cinematic => "Free look. Future versions will add FOV and orbit effects.",
            Self::CombatCam => "Slight downward tilt for combat awareness. Press F5 for third-person.",
        }
    }
}

/// Per-player camera state.
pub struct CameraState {
    pub mode: CameraMode,
}

pub struct CameraManager {
    cameras: Mutex<HashMap<i32, CameraState>>,  // player entity_id -> state
}

impl CameraManager {
    pub fn new() -> Self {
        Self {
            cameras: Mutex::new(HashMap::new()),
        }
    }

    /// Switch `player`'s camera to `mode`.
    pub async fn set_mode(&self, player: &Arc<pumpkin::entity::player::Player>, mode: CameraMode) {
        let player_eid = player.entity_id();

        // Record the new mode
        self.cameras.lock().unwrap().insert(player_eid, CameraState { mode });

        // If the mode forces a pitch, apply it immediately
        if let Some(pitch) = mode.forced_pitch() {
            let (yaw, _) = player.rotation();
            player.living_entity.entity.set_rotation(yaw, pitch);
            player.living_entity.entity.send_pos_rot();
        }

        // No CSetCamera packet — we don't spectate anything. The player
        // keeps their own camera and can move/rotate freely.
    }

    /// Remove the camera for `player_entity_id`.
    pub fn remove_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().remove(&player_entity_id)
    }

    /// Get the camera mode for `player_entity_id`.
    pub fn get_camera_mode(&self, player_entity_id: i32) -> Option<CameraMode> {
        self.cameras.lock().unwrap().get(&player_entity_id).map(|s| s.mode)
    }

    /// Per-tick update: force pitch for modes that require it.
    /// Called from ServerTickStartEvent handler.
    pub async fn tick_all(&self, server: &Arc<Server>) {
        let snapshots: Vec<(i32, CameraMode)> = {
            let cams = self.cameras.lock().unwrap();
            cams.iter().map(|(eid, state)| (*eid, state.mode)).collect()
        };

        for (player_eid, mode) in snapshots {
            // Only force pitch for modes that need it
            let Some(forced_pitch) = mode.forced_pitch() else { continue; };

            // Find the player
            let player_opt = find_player_by_entity_id(server, player_eid).await;
            let Some(player) = player_opt else {
                // Player gone — clean up
                self.remove_camera(player_eid);
                continue;
            };

            // Force the pitch but keep the player's yaw (horizontal rotation)
            let (yaw, _) = player.rotation();
            player.living_entity.entity.set_rotation(yaw, forced_pitch);
            // Note: we don't call send_pos_rot() every tick because that would
            // fight with the client's own rotation updates. The client will
            // smoothly adjust to the forced pitch.
        }
    }
}

/// Global camera manager.
pub static CAMERA_MANAGER: LazyLock<CameraManager> = LazyLock::new(CameraManager::new);

/// Find a player by entity_id across all worlds.
async fn find_player_by_entity_id(server: &Arc<Server>, entity_id: i32) -> Option<Arc<pumpkin::entity::player::Player>> {
    for world in server.worlds.load().iter() {
        for player in world.players.load().iter() {
            if player.entity_id() == entity_id {
                return Some(player.clone());
            }
        }
    }
    None
}

/// Clean up all cameras (called on plugin unload).
pub async fn cleanup_all_cameras(server: &Arc<Server>) {
    let player_eids: Vec<i32> = CAMERA_MANAGER.cameras.lock().unwrap().keys().copied().collect();
    for eid in player_eids {
        // Reset to first person
        if let Some(player) = find_player_by_entity_id(server, eid).await {
            CAMERA_MANAGER.set_mode(&player, CameraMode::FirstPerson).await;
        }
        CAMERA_MANAGER.remove_camera(eid);
    }
}
