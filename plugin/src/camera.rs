use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use uuid;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_protocol::java::client::play::{CSpawnEntity, CTeleportEntity};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::PositionFlag;

/// Camera modes with position offsets relative to the player.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
            "firstperson" | "first_person" | "fp" => Some(Self::FirstPerson),
            "overshoulder" | "over_shoulder" | "os" => Some(Self::OverShoulder),
            "topdown" | "top_down" | "td" => Some(Self::TopDown),
            "cinematic" | "cin" => Some(Self::Cinematic),
            "combatcam" | "combat_cam" | "cc" => Some(Self::CombatCam),
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

    /// Returns (offset, pitch_offset) in blocks. Offset is relative to player position
    /// rotated by the player's yaw.
    pub fn offset(&self) -> (f64, f64, f64, f32) {
        // (x_offset, y_offset, z_offset, pitch_offset_degrees)
        match self {
            Self::FirstPerson => (0.0, 0.0, 0.0, 0.0),
            Self::OverShoulder => (-1.5, 1.0, -2.0, 10.0),
            Self::TopDown => (0.0, 12.0, 0.0, 90.0),
            Self::Cinematic => (-3.0, 2.0, -4.0, 15.0),
            Self::CombatCam => (-2.0, 1.5, -2.5, 5.0),
        }
    }
}

pub struct CameraState {
    pub mode: CameraMode,
    pub fake_entity_id: i32,
    pub fake_uuid: uuid::Uuid,
}

impl Clone for CameraState {
    fn clone(&self) -> Self {
        Self {
            mode: self.mode,
            fake_entity_id: self.fake_entity_id,
            fake_uuid: self.fake_uuid,
        }
    }
}
impl Copy for CameraState {}

pub struct CameraManager {
    cameras: Mutex<HashMap<i32, CameraState>>, // player entity_id -> CameraState
    next_fake_id: Mutex<i32>,
}

impl CameraManager {
    pub fn new() -> Self {
        Self {
            cameras: Mutex::new(HashMap::new()),
            next_fake_id: Mutex::new(-1000),
        }
    }

    fn alloc_fake_id(&self) -> i32 {
        let mut id = self.next_fake_id.lock().unwrap();
        let val = *id;
        *id -= 1;
        val
    }

    pub fn set_camera_mode(&self, player_entity_id: i32, mode: CameraMode) -> i32 {
        let mut cams = self.cameras.lock().unwrap();
        let fake_id = self.alloc_fake_id();
        let fake_uuid = uuid::Uuid::new_v4();
        cams.insert(player_entity_id, CameraState {
            mode,
            fake_entity_id: fake_id,
            fake_uuid,
        });
        fake_id
    }

    pub fn get_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().get(&player_entity_id).copied()
    }

    pub fn remove_camera(&self, player_entity_id: i32) -> Option<CameraState> {
        self.cameras.lock().unwrap().remove(&player_entity_id)
    }

    pub async fn remove_all(&self) {
        let cams = self.cameras.lock().unwrap().drain().collect::<Vec<_>>();
        // Note: we can't send packets here without access to the client.
        // The tick handler should clean up per-player.
        drop(cams);
    }
}

pub static CAMERA_MANAGER: LazyLock<CameraManager> = LazyLock::new(CameraManager::new);

/// Build the CSpawnEntity packet for an invisible armor stand.
/// Armor stand entity type ID = 2 (in modern MC entity type registry).
pub fn build_spawn_packet(camera_state: &CameraState, pos: Vector3<f64>, yaw: f32) -> CSpawnEntity {
    CSpawnEntity::new(
        VarInt(camera_state.fake_entity_id),
        camera_state.fake_uuid,
        VarInt(2), // Armor Stand entity type
        pos,
        0.0,    // pitch
        yaw,    // yaw
        yaw,    // head_yaw
        VarInt(0), // data
        Vector3::new(0.0, 0.0, 0.0), // velocity
    )
}

/// Build the CTeleportEntity packet to move the camera entity.
pub fn build_teleport_packet(camera_state: &CameraState, pos: Vector3<f64>, yaw: f32, pitch: f32) -> CTeleportEntity<'static> {
    // Use a const slice so it has 'static lifetime; we cannot borrow a
    // local variable here because CTeleportEntity<'static> outlives this fn.
    const EMPTY_RELATIVES: &[PositionFlag] = &[];
    CTeleportEntity::new(
        VarInt(camera_state.fake_entity_id),
        pos,
        Vector3::new(0.0, 0.0, 0.0),
        yaw,
        pitch,
        EMPTY_RELATIVES,
        false,
    )
}

/// Calculate camera position from player position, yaw, and camera mode offset.
/// The offset is rotated around the Y axis by the player's yaw.
pub fn calculate_camera_pos(
    player_pos: Vector3<f64>,
    player_yaw: f32,
    mode: &CameraMode,
) -> (Vector3<f64>, f32) {
    let (ox, oy, oz, pitch_off) = mode.offset();

    // For FirstPerson, just return player position
    if matches!(mode, CameraMode::FirstPerson) {
        return (player_pos, player_yaw);
    }

    // Rotate the XZ offset by yaw
    let yaw_rad = f64::from(player_yaw) * std::f64::consts::PI / 180.0;
    let cos_y = yaw_rad.cos();
    let sin_y = yaw_rad.sin();

    let rx = ox * cos_y - oz * sin_y;
    let rz = ox * sin_y + oz * cos_y;

    let cam_pos = Vector3::new(
        player_pos.x + rx,
        player_pos.y + oy,
        player_pos.z - rz, // MC z is inverted
    );

    // Camera yaw = player yaw (looking same direction)
    // Camera pitch = player pitch + offset
    let cam_yaw = player_yaw;
    let cam_pitch = pitch_off; // simplified; could add player pitch

    (cam_pos, cam_yaw)
}
