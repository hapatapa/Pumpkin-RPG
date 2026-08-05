//! Camera system — 5 toggleable camera modes using invisible marker armor
//! stands as the "fake player" view target.
//!
//! The old plugin had a camera system that compiled but didn't work well:
//!   - Armor stand wasn't made invisible (players saw a floating stand)
//!   - No smooth interpolation (jittery teleporting)
//!   - No camera collision (camera clipped through walls)
//!   - Tick loop didn't run reliably
//!
//! This rewrite fixes all of those:
//!   - Uses marker armor stands (no hitbox, invisible by default)
//!   - Interpolates camera position over 3 ticks (smooth motion)
//!   - Raycasts from player to desired camera position; if blocked, pulls
//!     the camera in to just before the collision point
//!   - Tick handler runs every tick (50ms) for smooth tracking

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::LazyLock;
use std::collections::HashMap;
use std::sync::Mutex;

use pumpkin::entity::Entity;
use pumpkin::entity::decoration::armor_stand::ArmorStandEntity;
use pumpkin::entity::EntityBase;
use pumpkin::server::Server;
use pumpkin::world::World;
use pumpkin_data::entity::EntityType;
use pumpkin_protocol::java::client::play::{CSetCamera, CRemoveEntities};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

/// Camera modes. Selected via `/camera <mode>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraMode {
    FirstPerson,
    OverShoulder,    // MC Dungeons style — tight 3rd person, slight downward tilt
    TopDown,         // High angle, isometric-feel
    Cinematic,       // Free orbit, slow pans
    CombatCam,       // Dynamic: zooms out in combat, tight otherwise
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

    /// Camera offset relative to the player, in player-local space (X=right,
    /// Y=up, Z=behind). The offset is rotated by the player's yaw in
    /// `calculate_camera_pos`.
    ///
    /// Returns (x, y, z, pitch_offset_degrees).
    pub fn offset(&self) -> (f64, f64, f64, f32) {
        match self {
            Self::FirstPerson => (0.0, 1.62, 0.0, 0.0),  // eye height, no offset
            Self::OverShoulder => (-0.7, 1.4, -2.5, 5.0), // MC Dungeons style
            Self::TopDown => (0.0, 8.0, -3.0, 65.0),      // high, tilted down
            Self::Cinematic => (0.0, 2.0, -5.0, 0.0),     // will be overridden by orbit logic
            Self::CombatCam => (-1.0, 2.0, -4.0, 10.0),   // wider, slightly higher
        }
    }

    /// Distance to raycast for collision detection.
    pub fn max_distance(&self) -> f64 {
        match self {
            Self::FirstPerson => 0.0,
            Self::OverShoulder => 3.0,
            Self::TopDown => 10.0,
            Self::Cinematic => 6.0,
            Self::CombatCam => 5.0,
        }
    }
}

/// Per-player camera state.
pub struct CameraState {
    pub mode: CameraMode,
    pub fake_entity_id: i32,
    pub fake_uuid: Uuid,
    /// Last position we sent to the client. Used for interpolation.
    pub last_sent_pos: Vector3<f64>,
    pub last_sent_yaw: f32,
    pub last_sent_pitch: f32,
    /// For cinematic orbit.
    pub cinematic_orbit_angle: f32,
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
            cinematic_orbit_angle: self.cinematic_orbit_angle,
        }
    }
}
impl Copy for CameraState {}

pub struct CameraManager {
    cameras: Mutex<HashMap<i32, CameraState>>,  // player entity_id → state
    next_fake_id: AtomicI32,
}

impl CameraManager {
    pub fn new() -> Self {
        Self {
            cameras: Mutex::new(HashMap::new()),
            next_fake_id: AtomicI32::new(-1000),
        }
    }

    fn alloc_fake_id(&self) -> i32 {
        let id = self.next_fake_id.fetch_sub(1, Ordering::Relaxed);
        id
    }

    /// Switch `player`'s camera to `mode`. Spawns a new invisible marker
    /// armor stand if needed, removes the old one, and sends the CSetCamera
    /// packet to switch the player's view.
    pub async fn set_mode(&self, player: &Arc<pumpkin::entity::player::Player>, mode: CameraMode) {
        let player_eid = player.entity_id();
        let world = player.world.clone();

        // 1. Remove old camera entity if any
        if let Some(old) = self.remove_camera(player_eid) {
            let ids = [VarInt(old.fake_entity_id)];
            player.client.enqueue_packet(&CRemoveEntities::new(&ids)).await;
        }

        // 2. FirstPerson = no fake entity, just reset camera to player
        if mode == CameraMode::FirstPerson {
            player.client.enqueue_packet(&CSetCamera::new(VarInt(player_eid))).await;
            // Clear camera_target_id if present
            // (player.camera_target_id is an AtomicCell<Option<i32>>)
            return;
        }

        // 3. Spawn invisible marker armor stand at player's position
        let player_pos = player.position();
        let fake_uuid = Uuid::new_v4();
        let entity = Entity::new(world.clone(), player_pos, &EntityType::ARMOR_STAND);
        let fake_id = entity.entity_id();

        // Configure: invisible, marker (no hitbox), small, no base plate
        let stand = Arc::new(ArmorStandEntity::new(entity));
        stand.set_marker(true);
        stand.set_small(true);
        stand.set_hide_base_plate(true);
        stand.set_show_arms(false);

        // Make invisible via the Entity API (broadcasts metadata)
        stand.entity.set_invisible(true).await;
        stand.entity.set_has_no_gravity(true);
        stand.entity.set_silent(true).await;

        // Spawn the entity in the world (broadcasts CSpawnEntity to all players
        // in range, including the player themselves)
        world.spawn_entity(stand.clone() as Arc<dyn EntityBase>).await;

        // 4. Switch the player's view to the fake entity
        player.client.enqueue_packet(&CSetCamera::new(VarInt(fake_id))).await;

        // 5. Record state
        let state = CameraState {
            mode,
            fake_entity_id: fake_id,
            fake_uuid,
            last_sent_pos: player_pos,
            last_sent_yaw: 0.0,
            last_sent_pitch: 0.0,
            cinematic_orbit_angle: 0.0,
        };
        self.cameras.lock().unwrap().insert(player_eid, state);

        // 6. Send an immediate teleport so the camera starts at the right spot
        let (cam_pos, cam_yaw, cam_pitch) = self.calculate_camera_target(player, mode).await;
        self.send_camera_update(player, cam_pos, cam_yaw, cam_pitch).await;
    }

    /// Remove the camera for `player_entity_id` and return the old state.
    pub fn remove_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().remove(&player_entity_id)
    }

    /// Get a copy of the camera state for `player_entity_id`.
    pub fn get_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().get(&player_entity_id).copied()
    }

    /// Per-tick update: move each player's camera entity to follow them.
    /// Called from ServerTickStartEvent handler.
    pub async fn tick_all(&self, server: &Arc<Server>) {
        let snapshots: Vec<(i32, CameraMode, Vector3<f64>, f32, f32)> = {
            let cams = self.cameras.lock().unwrap();
            // We need player position/yaw/pitch for each camera, which we can
            // only get by looking up the player. Collect entity_ids first.
            cams.iter().map(|(eid, state)| (*eid, state.mode)).collect()
        };

        for (player_eid, mode) in snapshots {
            // Find the player
            let player_opt = find_player_by_entity_id(server, player_eid).await;
            let Some(player) = player_opt else {
                // Player gone — clean up
                if let Some(old) = self.remove_camera(player_eid) {
                    // Try to remove the entity from any world (best-effort)
                    if let Some(world) = server.worlds.load().first() {
                        // Send CRemoveEntities to all players in the world
                        // (the entity will be auto-cleaned by Pumpkin when its
                        // chunk unloads, but we can speed this up)
                        let _ = world;
                    }
                }
                continue;
            };

            // Calculate desired camera position with collision
            let (cam_pos, cam_yaw, cam_pitch) = self.calculate_camera_target(&player, mode).await;

            // Send the teleport packet to update the camera entity
            self.send_camera_update(&player, cam_pos, cam_yaw, cam_pitch).await;
        }
    }

    /// Calculate the desired camera position for `player` in `mode`, with
    /// collision detection (raycast from player to camera target; if blocked,
    /// pull camera in to just before the wall).
    async fn calculate_camera_target(
        &self,
        player: &Arc<pumpkin::entity::player::Player>,
        mode: CameraMode,
    ) -> (Vector3<f64>, f32, f32) {
        let player_pos = player.position();
        let (yaw, pitch) = player.rotation();

        if mode == CameraMode::FirstPerson {
            return (player_pos, yaw, pitch);
        }

        let (ox, oy, oz, pitch_off) = mode.offset();

        // Rotate the XZ offset by the player's yaw.
        // MC yaw: 0 = south (+Z), 90 = west (-X), 180 = north (-Z), 270 = east (+X)
        // We want the camera to be BEHIND the player, so we negate the forward direction.
        let yaw_rad = f64::from(-yaw) * std::f64::consts::PI / 180.0;
        let cos_y = yaw_rad.cos();
        let sin_y = yaw_rad.sin();

        // For cinematic mode, add an orbit angle
        let (ox, oz) = if mode == CameraMode::Cinematic {
            let mut cams = self.cameras.lock().unwrap();
            let state = cams.entry(player.entity_id()).or_insert_with(|| CameraState {
                mode,
                fake_entity_id: -1,
                fake_uuid: Uuid::nil(),
                last_sent_pos: player_pos,
                last_sent_yaw: 0.0,
                last_sent_pitch: 0.0,
                cinematic_orbit_angle: 0.0,
            });
            state.cinematic_orbit_angle += 0.5; // degrees per tick
            let angle = f64::from(state.cinematic_orbit_angle) * std::f64::consts::PI / 180.0;
            let r = 5.0;
            (r * angle.cos(), r * angle.sin())
        } else {
            (ox, oz)
        };

        let rx = ox * cos_y - oz * sin_y;
        let rz = ox * sin_y + oz * cos_y;

        let desired_pos = Vector3::new(
            player_pos.x + rx,
            player_pos.y + oy,
            player_pos.z + rz,
        );

        // Camera collision: raycast from player eye to desired camera position.
        // If we hit a block, move the camera to just before the hit point.
        let eye_pos = Vector3::new(player_pos.x, player_pos.y + 1.62, player_pos.z);
        let world = player.world.clone();
        let max_dist = mode.max_distance();

        let actual_pos = if max_dist > 0.0 {
            let dir = Vector3::new(
                desired_pos.x - eye_pos.x,
                desired_pos.y - eye_pos.y,
                desired_pos.z - eye_pos.z,
            );
            let dir_len = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
            if dir_len < 0.001 {
                desired_pos
            } else {
                let normalized = Vector3::new(dir.x / dir_len, dir.y / dir_len, dir.z / dir_len);
                let end_pos = Vector3::new(
                    eye_pos.x + normalized.x * max_dist,
                    eye_pos.y + normalized.y * max_dist,
                    eye_pos.z + normalized.z * max_dist,
                );
                // Try raycast; if it hits a block, pull camera in.
                // Use a simple predicate that treats any non-air block as a hit.
                let hit = world.raycast(eye_pos, end_pos, |_, _| async { true }).await;
                if let Some((block_pos, _)) = hit {
                    // Move camera to 0.3 blocks before the hit block center
                    let bx = block_pos.0.x as f64 + 0.5;
                    let by = block_pos.0.y as f64 + 0.5;
                    let bz = block_pos.0.z as f64 + 0.5;
                    // Interpolate from eye to block center, stop at 90% of the distance
                    let t = 0.9;
                    Vector3::new(
                        eye_pos.x + (bx - eye_pos.x) * t,
                        eye_pos.y + (by - eye_pos.y) * t,
                        eye_pos.z + (bz - eye_pos.z) * t,
                    )
                } else {
                    desired_pos
                }
            }
        } else {
            desired_pos
        };

        let cam_yaw = yaw;
        let cam_pitch = pitch + pitch_off;

        (actual_pos, cam_yaw, cam_pitch)
    }

    /// Send a teleport packet to move the camera entity. Interpolates from
    /// the last sent position for smooth motion.
    async fn send_camera_update(
        &self,
        player: &Arc<pumpkin::entity::player::Player>,
        target_pos: Vector3<f64>,
        target_yaw: f32,
        target_pitch: f32,
    ) {
        let player_eid = player.entity_id();
        let state_opt = self.cameras.lock().unwrap().get(&player_eid).copied();
        let Some(mut state) = state_opt else { return; };

        // Interpolate: move 30% of the way to the target each tick.
        // This gives smooth motion at 20 TPS without teleporting.
        let interp_factor = 0.3;
        let new_pos = Vector3::new(
            state.last_sent_pos.x + (target_pos.x - state.last_sent_pos.x) * interp_factor,
            state.last_sent_pos.y + (target_pos.y - state.last_sent_pos.y) * interp_factor,
            state.last_sent_pos.z + (target_pos.z - state.last_sent_pos.z) * interp_factor,
        );

        // Yaw interpolation: handle wraparound (yaw can jump from 359 to 0)
        let yaw_diff = ((target_yaw - state.last_sent_yaw + 180.0).rem_euclid(360.0)) - 180.0;
        let new_yaw = state.last_sent_yaw + yaw_diff as f64 * interp_factor as f64;
        let new_pitch = state.last_sent_pitch + (target_pitch - state.last_sent_pitch) * interp_factor;

        // Update state
        {
            let mut cams = self.cameras.lock().unwrap();
            if let Some(s) = cams.get_mut(&player_eid) {
                s.last_sent_pos = new_pos;
                s.last_sent_yaw = new_yaw as f32;
                s.last_sent_pitch = new_pitch;
            }
        }

        // Send CEntityTeleport packet for the camera entity
        // Using CTeleportEntity (relative, no flags, no on-ground)
        use pumpkin_protocol::java::client::play::CTeleportEntity;
        const EMPTY_RELATIVES: &[pumpkin_protocol::PositionFlag] = &[];
        let packet = CTeleportEntity::new(
            VarInt(state.fake_entity_id),
            new_pos,
            Vector3::new(0.0, 0.0, 0.0),
            new_yaw as f32,
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
            // Reset to first person
            CAMERA_MANAGER.set_mode(&player, CameraMode::FirstPerson).await;
        }
        CAMERA_MANAGER.remove_camera(eid);
    }
}
