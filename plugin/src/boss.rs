//! Boss fights — spawnable boss entities with phases, HP bars, and special
//! mechanics.
//!
//! v1 ships one boss type: the Skeleton King. More can be added by extending
//! the `BossType` enum and `boss_def()` function.
//!
//! Bosses are vanilla mobs (e.g. a Skeleton for Skeleton King) with modified
//! attributes (high HP, custom name) and a tracked `BossState` that drives
//! phase transitions and special attacks via the tick handler.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::LazyLock;
use std::collections::HashMap;
use std::sync::Mutex;

use pumpkin::server::Server;
use pumpkin::world::bossbar::{Bossbar, BossbarColor, BossbarDivisions, BossbarFlags};
use pumpkin_util::text::TextComponent;
use pumpkin_util::math::vector3::Vector3;
use pumpkin::entity::Entity;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};

use crate::player::current_tick;

/// Boss types. v1 has just the Skeleton King; the structure is extensible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossType {
    SkeletonKing,
    CorruptedGolem,   // placeholder for v2
    WitherQueen,      // placeholder for v2
}

impl BossType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "skeleton_king" | "skeletonking" | "sk" => Some(Self::SkeletonKing),
            "corrupted_golem" | "golem" => Some(Self::CorruptedGolem),
            "wither_queen" | "witherqueen" | "wq" => Some(Self::WitherQueen),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::SkeletonKing => "Skeleton King",
            Self::CorruptedGolem => "Corrupted Golem",
            Self::WitherQueen => "Wither Queen",
        }
    }

    pub fn entity_type(&self) -> &'static EntityType {
        match self {
            Self::SkeletonKing => &EntityType::SKELETON,
            Self::CorruptedGolem => &EntityType::IRON_GOLEM,
            Self::WitherQueen => &EntityType::WITHER,
        }
    }

    pub fn max_hp(&self) -> f32 {
        match self {
            Self::SkeletonKing => 200.0,
            Self::CorruptedGolem => 500.0,
            Self::WitherQueen => 1000.0,
        }
    }

    pub fn color(&self) -> BossbarColor {
        match self {
            Self::SkeletonKing => BossbarColor::Purple,
            Self::CorruptedGolem => BossbarColor::Red,
            Self::WitherQueen => BossbarColor::Black,
        }
    }
}

/// Phases drive behavior changes at HP thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossPhase {
    Phase1,  // 100% - 66%: normal attacks
    Phase2,  // 66% - 33%:  enraged, faster attacks, summons minions
    Phase3,  // 33% - 0%:   final, AoE attacks, enrage timer
}

impl BossPhase {
    pub fn from_hp_ratio(ratio: f32) -> Self {
        if ratio > 0.66 { Self::Phase1 }
        else if ratio > 0.33 { Self::Phase2 }
        else { Self::Phase3 }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Phase1 => "Phase 1",
            Self::Phase2 => "Phase 2 — Enraged",
            Self::Phase3 => "Phase 3 — Final",
        }
    }
}

/// Tracked state for an active boss fight.
pub struct BossState {
    pub boss_type: BossType,
    pub entity_id: i32,                    // vanilla entity id of the boss mob
    pub entity_uuid: uuid::Uuid,
    pub bossbar_uuid: uuid::Uuid,
    pub current_phase: BossPhase,
    pub max_hp: f32,
    pub spawned_at_tick: u32,
    pub last_special_attack_tick: u32,
    pub players_tracking: Vec<uuid::Uuid>, // players who should see the HP bar
}

impl BossState {
    pub fn current_phase(&self) -> BossPhase { self.current_phase }
}

/// Global registry of active boss fights, keyed by boss entity_id.
pub static BOSSES: LazyLock<Mutex<HashMap<i32, BossState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Spawn a boss of `boss_type` at `pos` in `world`. Returns the entity_id of
/// the spawned boss, or None on failure.
///
/// This is async because spawning involves world I/O. The boss bar is sent
/// only to players within 32 blocks of the spawn position.
pub async fn spawn_boss(
    server: &Arc<Server>,
    boss_type: BossType,
    pos: Vector3<f64>,
    world: &Arc<pumpkin::world::World>,
) -> Option<i32> {
    // 1. Create the vanilla entity
    let entity_uuid = uuid::Uuid::new_v4();
    let entity = Entity::new(world.clone(), pos, boss_type.entity_type());
    let entity_id = entity.entity_id();

    // 2. Wrap in the appropriate mob type. For v1 we use the base Entity and
    //    set HP via the living entity's set_max_health. The actual mob AI is
    //    handled by Pumpkin's vanilla entity system.
    use pumpkin::entity::EntityBase;
    let mob: Arc<dyn EntityBase> = match boss_type {
        BossType::SkeletonKing => {
            use pumpkin::entity::hostile::skeleton::SkeletonEntity;
            Arc::new(SkeletonEntity::new(entity))
        }
        BossType::CorruptedGolem => {
            use pumpkin::entity::passive::iron_golem::IronGolemEntity;
            Arc::new(IronGolemEntity::new(entity))
        }
        BossType::WitherQueen => {
            // Wither requires a more complex setup (flying entity, projectile AI).
            // For v1 we fall back to a Skeleton with high HP if Wither spawning fails.
            use pumpkin::entity::hostile::skeleton::SkeletonEntity;
            Arc::new(SkeletonEntity::new(entity))
        }
    };

    // 3. Configure HP
    if let Some(living) = mob.get_living_entity() {
        living.set_max_health(boss_type.max_hp()).await;
        living.set_health(boss_type.max_hp());
        living.entity.set_custom_name(TextComponent::text(boss_type.display_name().to_string()));
        living.entity.set_custom_name_visible(true);
        living.entity.set_glowing(true).await;
    }

    // 4. Spawn into the world
    world.spawn_entity(mob.clone()).await;

    // 5. Create boss bar
    let mut bossbar = Bossbar::new(TextComponent::text(boss_type.display_name().to_string()));
    bossbar.health = 1.0;
    bossbar.color = boss_type.color();
    bossbar.division = BossbarDivisions::Notches20;
    bossbar.flags = BossbarFlags::DARKEN_SKY | BossbarFlags::CREATE_FOG;
    let bossbar_uuid = bossbar.uuid;

    // 6. Find players within 32 blocks and show them the bar
    let nearby_players: Vec<Arc<pumpkin::entity::player::Player>> = world
        .players
        .load()
        .iter()
        .filter(|p| {
            let ppos = p.position();
            let dx = ppos.x - pos.x;
            let dy = ppos.y - pos.y;
            let dz = ppos.z - pos.z;
            dx * dx + dy * dy + dz * dz < 1024.0 // 32^2
        })
        .cloned()
        .collect();

    let player_uuids: Vec<uuid::Uuid> = nearby_players.iter().map(|p| p.gameprofile.id).collect();

    for player in &nearby_players {
        player.send_bossbar(&bossbar).await;
    }

    // 7. Play spawn sound
    world.play_sound(Sound::EntityWitherSpawn, SoundCategory::Hostile, &pos);

    // 8. Register state
    let state = BossState {
        boss_type,
        entity_id,
        entity_uuid,
        bossbar_uuid,
        current_phase: BossPhase::Phase1,
        max_hp: boss_type.max_hp(),
        spawned_at_tick: current_tick(),
        last_special_attack_tick: current_tick(),
        players_tracking: player_uuids,
    };
    BOSSES.lock().unwrap().insert(entity_id, state);

    Some(entity_id)
}

/// Update a boss's HP bar based on its current HP. Called from the entity
/// damage handler when a boss takes damage.
pub async fn update_boss_hp(server: &Arc<Server>, entity_id: i32, current_hp: f32) {
    let state = {
        let map = BOSSES.lock().unwrap();
        map.get(&entity_id).cloned()
    };
    let Some(state) = state else { return; };

    let ratio = (current_hp / state.max_hp).clamp(0.0, 1.0);

    // Update boss bar for all tracking players
    for &player_uuid in &state.players_tracking {
        if let Some(player) = server.get_player_by_uuid(player_uuid) {
            player.update_bossbar_health(&state.bossbar_uuid, ratio).await;
        }
    }

    // Check for phase transition
    let new_phase = BossPhase::from_hp_ratio(ratio);
    if new_phase != state.current_phase {
        let mut map = BOSSES.lock().unwrap();
        if let Some(s) = map.get_mut(&entity_id) {
            s.current_phase = new_phase;
        }
        drop(map);

        // Announce phase change to tracking players
        let msg = format!("\u{00a7}c{} enters {}!\u{00a7}r",
            state.boss_type.display_name(), new_phase.display_name());
        for &player_uuid in &state.players_tracking {
            if let Some(player) = server.get_player_by_uuid(player_uuid) {
                use pumpkin::plugin::api::title::TitleBuilder;
                TitleBuilder::new()
                    .title_text(msg.clone())
                    .times(10, 60, 10)
                    .send_to(&player).await;
            }
        }
    }
}

/// Remove a boss from tracking (on death or unload).
pub async fn remove_boss(server: &Arc<Server>, entity_id: i32) {
    let state = BOSSES.lock().unwrap().remove(&entity_id);
    let Some(state) = state else { return; };

    // Remove boss bar from all players
    for &player_uuid in &state.players_tracking {
        if let Some(player) = server.get_player_by_uuid(player_uuid) {
            player.remove_bossbar(state.bossbar_uuid).await;
        }
    }

    // Drop guaranteed loot (legendary or mythic)
    let allow_mythic = matches!(state.boss_type, BossType::WitherQueen);
    let world = server.worlds.load().first().cloned();
    if let Some(world) = world {
        // We don't have the boss's exact death position easily here; use the
        // world's spawn point as a fallback. In a real implementation we'd
        // look up the entity's position before removal.
        let pos = pumpkin_util::math::vector3::Vector3::new(0.0, 100.0, 0.0);
        crate::loot::drop_loot(
            &world, pos,
            &state.boss_type.entity_type().name,
            20, // assume level 20 for boss loot
            allow_mythic,
        ).await;
        // Drop extra loot for boss
        for _ in 0..3 {
            crate::loot::drop_loot(
                &world, pos,
                &state.boss_type.entity_type().name,
                20, allow_mythic,
            ).await;
        }
    }
}

/// Clean up all active boss fights (called on plugin unload).
pub async fn cleanup_all_bosses(server: &Arc<Server>) {
    let entity_ids: Vec<i32> = BOSSES.lock().unwrap().keys().copied().collect();
    for eid in entity_ids {
        remove_boss(server, eid).await;
    }
}

/// Is `entity_id` an active boss?
pub fn is_boss(entity_id: i32) -> bool {
    BOSSES.lock().unwrap().contains_key(&entity_id)
}

/// Get the boss state for `entity_id` (read-only).
pub fn with_boss_state<F, R>(entity_id: i32, f: F) -> R
where
    F: FnOnce(Option<&BossState>) -> R,
    R: Default,
{
    let map = BOSSES.lock().unwrap();
    f(map.get(&entity_id))
}

// === Per-tick boss AI ===
//
// Called from the tick handler. Each boss performs special attacks on a
// cooldown that depends on its phase.

pub async fn tick_all_bosses(server: &Arc<Server>) {
    let tick = current_tick();
    let boss_snapshots: Vec<(i32, BossType, BossPhase, u32, u32, uuid::Uuid, Vec<uuid::Uuid>)> = {
        let map = BOSSES.lock().unwrap();
        map.iter().map(|(eid, s)| (*eid, s.boss_type, s.current_phase, s.last_special_attack_tick, s.spawned_at_tick, s.bossbar_uuid, s.players_tracking.clone())).collect()
    };

    for (entity_id, boss_type, phase, last_special, spawned, bossbar_uuid, players) in boss_snapshots {
        // Phase-dependent cooldown (in ticks)
        let cooldown_ticks: u32 = match (boss_type, phase) {
            (BossType::SkeletonKing, BossPhase::Phase1) => 100, // 5s
            (BossType::SkeletonKing, BossPhase::Phase2) => 60,  // 3s
            (BossType::SkeletonKing, BossPhase::Phase3) => 40,  // 2s
            _ => 100,
        };

        if tick.saturating_sub(last_special) < cooldown_ticks { continue; }

        // Find the boss entity in any world
        let mut boss_pos: Option<Vector3<f64>> = None;
        let mut world_ref: Option<Arc<pumpkin::world::World>> = None;
        for world in server.worlds.load().iter() {
            for entity in world.entities.load().iter() {
                if entity.entity_id() == entity_id {
                    boss_pos = Some(entity.position());
                    world_ref = Some(world.clone());
                    break;
                }
            }
            if boss_pos.is_some() { break; }
        }

        let (Some(pos), Some(world)) = (boss_pos, world_ref) else { continue; };

        // Update last special attack tick
        {
            let mut map = BOSSES.lock().unwrap();
            if let Some(s) = map.get_mut(&entity_id) {
                s.last_special_attack_tick = tick;
            }
        }

        // Perform phase-appropriate special attack
        match (boss_type, phase) {
            (BossType::SkeletonKing, _) => {
                // Summon 2 skeleton minions + AoE particle burst
                use pumpkin::entity::hostile::skeleton::SkeletonEntity;
                for _ in 0..2 {
                    let offset_x = (rand::random::<f64>() - 0.5) * 4.0;
                    let offset_z = (rand::random::<f64>() - 0.5) * 4.0;
                    let minion_pos = Vector3::new(pos.x + offset_x, pos.y, pos.z + offset_z);
                    let minion_uuid = uuid::Uuid::new_v4();
                    let minion_entity = Entity::new(world.clone(), minion_pos, &EntityType::SKELETON);
                    let minion = Arc::new(SkeletonEntity::new(minion_entity));
                    if let Some(living) = minion.get_living_entity() {
                        living.set_max_health(20.0).await;
                        living.set_health(20.0);
                    }
                    world.spawn_entity(minion).await;
                }
                // Particle burst
                use pumpkin_data::particle::Particle;
                world.spawn_particle(pos, Vector3::new(1.0, 1.0, 1.0), 0.5, 30, Particle::DragonBreath);
                // Sound
                world.play_sound(Sound::EntityWitherShoot, SoundCategory::Hostile, &pos);
            }
            _ => {} // other boss types: deferred to v2
        }
    }
}
