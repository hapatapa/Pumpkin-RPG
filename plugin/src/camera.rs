//! Camera system — real third-person camera using invisible armor stands
//! and CSetCamera.
//!
//! How this works:
//!   1. Spawn an invisible marker armor stand (the "camera entity")
//!   2. Send CSetCamera to make the client render from the armor stand's
//!      perspective. This does NOT change the player's gamemode or
//!      movement — the client still sends position/rotation packets
//!      for the player, so WASD and mouse look work normally.
//!   3. Every tick, reposition the armor stand behind the player based
//!      on their position and yaw. The client sees the camera move,
//!      creating a third-person view.
//!
//! The key insight: CSetCamera only changes the RENDER perspective, not
//! the player's input/movement. The player retains full WASD movement
//! and mouse rotation. The server moves the camera entity to follow
//! the player, and the client's view follows the camera entity.
//!
//! This is the same technique used by servers like DiamondFire for
//! custom camera angles.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::LazyLock;
use std::collections::HashMap;
use std::sync::Mutex;

use pumpkin::entity::Entity;
use pumpkin::entity::decoration::armor_stand::ArmorStandEntity;
use pumpkin::entity::EntityBase;
use pumpkin::server::Server;
use pumpkin_data::entity::EntityType;
use pumpkin_protocol::java::client::play::{CSetCamera, CRemoveEntities, CTeleportEntity};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

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

    /// Camera offset relative to the player, in player-local space.
    /// (x=right, y=up, z=behind). Rotated by player yaw in calculate_target.
    /// Returns (x, y, z, pitch_offset_degrees).
    pub fn offset(&self) -> (f64, f64, f64, f32) {
        match self {
            Self::FirstPerson => (0.0, 0.0, 0.0, 0.0),
            Self::OverShoulder => (-0.7, 1.4, -2.5, 5.0),
            Self::TopDown => (0.0, 8.0, -3.0, 65.0),
            Self::Cinematic => (0.0, 2.0, -5.0, 0.0),
            Self::CombatCam => (-1.0, 2.0, -4.0, 10.0),
        }
    }
}

/// Per-player camera state.
pub struct CameraState {
    pub mode: CameraMode,
    pub fake_entity_id: i32,
    pub fake_uuid: Uuid,
    /// Last position we sent to the client (for interpolation).
    pub last_sent_pos: Vector3<f64>,
    pub last_sent_yaw: f32,
    pub last_sent_pitch: f32,
}

impl Clone for CameraState {
    fn clone(&self) -> Self {
        Self {
            mode: self.mode,
            fake_entity_id: self.fake_entity_id,
            fake_uuid: self.fake_uuid,
            last_sent_pos: self.last_sent_pos.clone(),
            last_sent_yaw: self.last_sent_yaw,
            last_sent_pitch: self.last_sent_pitch,
        }
    }
}
impl Copy for CameraState {}

pub struct CameraManager {
    cameras: Mutex<HashMap<i32, CameraState>>,
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
        let world = player.world().clone();

        // 1. Remove old camera entity if any
        if let Some(old) = self.remove_camera(player_eid) {
            let ids = [VarInt(old.fake_entity_id)];
            player.client.enqueue_packet(&CRemoveEntities::new(&ids)).await;
        }

        // 2. FirstPerson = no camera entity, reset view to player
        if mode == CameraMode::FirstPerson {
            player.client.enqueue_packet(&CSetCamera::new(VarInt(player_eid))).await;
            return;
        }

        // 3. Spawn invisible marker armor stand at player's position.
        let player_pos = player.position();
        let entity = Entity::new(world.clone(), player_pos, &EntityType::ARMOR_STAND);
        let fake_id = entity.entity_id;
        let fake_uuid = Uuid::new_v4();

        let stand = ArmorStandEntity::new(entity);
        stand.set_marker(true);
        stand.set_small(true);
        stand.set_hide_base_plate(true);
        stand.set_show_arms(false);

        let stand_entity = stand.get_entity();
        stand_entity.set_invisible(true).await;
        stand_entity.set_has_no_gravity(true);
        stand_entity.set_silent(true);

        let stand_arc: Arc<ArmorStandEntity> = Arc::new(stand);
        world.spawn_entity(Arc::clone(&stand_arc) as Arc<dyn EntityBase>).await;

        // 4. Send CSetCamera to make client render from the armor stand.
        // This does NOT change the player's gamemode or movement — the
        // client still sends position/rotation packets for the player.
        player.client.enqueue_packet(&CSetCamera::new(VarInt(fake_id))).await;

        // 5. Record state
        let state = CameraState {
            mode,
            fake_entity_id: fake_id,
            fake_uuid,
            last_sent_pos: player_pos,
            last_sent_yaw: 0.0,
            last_sent_pitch: 0.0,
        };
        self.cameras.lock().unwrap().insert(player_eid, state);

        // 6. Send immediate camera position update
        let (cam_pos, cam_yaw, cam_pitch) = self.calculate_camera_target(player, mode);
        self.send_camera_update(player, cam_pos, cam_yaw, cam_pitch).await;
    }

    pub fn remove_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().remove(&player_entity_id)
    }

    pub fn get_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().get(&player_entity_id).copied()
    }

    /// Per-tick update: reposition each player's camera entity to follow
    /// them. Called from ServerTickStartEvent handler every tick (50ms).
    pub async fn tick_all(&self, server: &Arc<Server>) {
        let snapshots: Vec<i32> = {
            let cams = self.cameras.lock().unwrap();
            cams.keys().copied().collect()
        };

        for player_eid in snapshots {
            let player_opt = find_player_by_entity_id(server, player_eid).await;
            let Some(player) = player_opt else {
                // Player gone — the LeaveHandler should have cleaned up,
                // but just in case:
                self.remove_camera(player_eid);
                continue;
            };

            let mode = {
                let cams = self.cameras.lock().unwrap();
                cams.get(&player_eid).map(|s| s.mode)
            };
            let Some(mode) = mode else { continue; };

            let (cam_pos, cam_yaw, cam_pitch) = self.calculate_camera_target(&player, mode);
            self.send_camera_update(&player, cam_pos, cam_yaw, cam_pitch).await;
        }
    }

    /// Calculate where the camera entity should be, based on the player's
    /// current position and yaw.
    fn calculate_camera_target(
        &self,
        player: &Arc<pumpkin::entity::player::Player>,
        mode: CameraMode,
    ) -> (Vector3<f64>, f32, f32) {
        let player_pos = player.position();
        let (yaw, pitch) = player.rotation();

        let (ox, oy, oz, pitch_off) = mode.offset();

        // Rotate the XZ offset by the player's yaw.
        // MC yaw: 0 = south (+Z), 90 = west (-X), etc.
        // We want the camera BEHIND the player, so we negate the forward direction.
        let yaw_rad = f64::from(-yaw) * std::f64::consts::PI / 180.0;
        let cos_y = yaw_rad.cos();
        let sin_y = yaw_rad.sin();

        let rx = ox * cos_y - oz * sin_y;
        let rz = ox * sin_y + oz * cos_y;

        let target_pos = Vector3::new(
            player_pos.x + rx,
            player_pos.y + oy,
            player_pos.z + rz,
        );

        // Camera yaw = player yaw (look same direction)
        // Camera pitch = player pitch + offset
        let cam_yaw = yaw;
        let cam_pitch = pitch + pitch_off;

        (target_pos, cam_yaw, cam_pitch)
    }

    /// Send a teleport packet to move the camera entity. Interpolates
    /// from the last sent position for smooth motion.
    async fn send_camera_update(
        &self,
        player: &Arc<pumpkin::entity::player::Player>,
        target_pos: Vector3<f64>,
        target_yaw: f32,
        target_pitch: f32,
    ) {
        let player_eid = player.entity_id();
        let state_opt = self.cameras.lock().unwrap().get(&player_eid).copied();
        let Some(state) = state_opt else { return; };

        // Interpolate: move 50% of the way to the target each tick.
        // Higher = snappier but potentially jittery; lower = smoother but laggy.
        let interp_f64: f64 = 0.5;
        let interp_f32: f32 = 0.5;

        let new_pos = Vector3::new(
            state.last_sent_pos.x + (target_pos.x - state.last_sent_pos.x) * interp_f64,
            state.last_sent_pos.y + (target_pos.y - state.last_sent_pos.y) * interp_f64,
            state.last_sent_pos.z + (target_pos.z - state.last_sent_pos.z) * interp_f64,
        );

        // Yaw interpolation with wraparound
        let yaw_diff = ((target_yaw - state.last_sent_yaw + 180.0).rem_euclid(360.0)) - 180.0;
        let new_yaw = state.last_sent_yaw + yaw_diff * interp_f32;
        let new_pitch = state.last_sent_pitch + (target_pitch - state.last_sent_pitch) * interp_f32;

        // Update state
        {
            let mut cams = self.cameras.lock().unwrap();
            if let Some(s) = cams.get_mut(&player_eid) {
                s.last_sent_pos = new_pos;
                s.last_sent_yaw = new_yaw;
                s.last_sent_pitch = new_pitch;
            }
        }

        // Send CTeleportEntity to move the camera entity
        const EMPTY_RELATIVES: &[pumpkin_protocol::PositionFlag] = &[];
        let packet = CTeleportEntity::new(
            VarInt(state.fake_entity_id),
            new_pos,
            Vector3::new(0.0, 0.0, 0.0),
            new_yaw,
            new_pitch,
            EMPTY_RELATIVES,
            false,
        );
        player.client.enqueue_packet(&packet).await;
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
        if let Some(player) = find_player_by_entity_id(server, eid).await {
            CAMERA_MANAGER.set_mode(&player, CameraMode::FirstPerson).await;
        }
        CAMERA_MANAGER.remove_camera(eid);
    }
}
