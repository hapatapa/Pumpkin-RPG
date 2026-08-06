//! Damage types and combat math.
//!
//! Six elemental damage types in an advantage cycle (like Pokemon types):
//!   Physical → Fire → Arcane → Holy → Dark → Frost → Physical
//! Each type deals 1.5x to the next type in the cycle, 0.75x to the previous.
//!
//! Class damage modifiers stack with elemental modifiers.

/// Six elemental damage types. Used for skill effects and class affinities.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RpgDamageType {
    Physical,
    Fire,
    Arcane,
    Holy,
    Dark,
    Frost,
}

impl RpgDamageType {
    /// The advantage cycle. Index N deals 1.5x to index N+1, 0.75x to N-1.
    const CYCLE: [Self; 6] = [
        Self::Physical,
        Self::Fire,
        Self::Arcane,
        Self::Holy,
        Self::Dark,
        Self::Frost,
    ];

    /// Returns the damage multiplier when `attacker` hits `defender`.
    /// 1.5 = advantage, 0.75 = disadvantage, 1.0 = neutral.
    /// Note: in practice we use this for mob-vs-mob elemental interactions.
    /// For player-vs-mob, the player's class type is compared to the mob's
    /// "native" element (if any).
    pub fn advantage_against(attacker: Self, defender: Self) -> f32 {
        let a = Self::CYCLE.iter().position(|&x| x == attacker).unwrap_or(0);
        let d = Self::CYCLE.iter().position(|&x| x == defender).unwrap_or(0);
        if (a + 1) % 6 == d {
            1.5
        } else if (d + 1) % 6 == a {
            0.75
        } else {
            1.0
        }
    }

    /// Minecraft color code for displaying this damage type in chat.
    pub fn color_code(&self) -> &'static str {
        match self {
            Self::Physical => "\u{00a7}7", // gray
            Self::Fire     => "\u{00a7}c", // red
            Self::Arcane   => "\u{00a7}b", // aqua
            Self::Holy     => "\u{00a7}e", // yellow
            Self::Dark     => "\u{00a7}8", // dark gray
            Self::Frost    => "\u{00a7}3", // dark aqua
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Physical => "Physical",
            Self::Fire     => "Fire",
            Self::Arcane   => "Arcane",
            Self::Holy     => "Holy",
            Self::Dark     => "Dark",
            Self::Frost    => "Frost",
        }
    }
}

// === Combat math ===

/// Compute the final damage dealt by a player to a mob, given:
///   - base_damage: vanilla MC damage (e.g. sword's attack_damage attribute)
///   - class_damage_mult: from the player's class (passive + level scaling)
///   - combo_mult: combo multiplier from consecutive hits (1.0 = no combo)
///   - skill_mult: skill damage multiplier if a skill is queued (1.0 = no skill)
///   - elemental_mult: elemental advantage multiplier (1.0 = neutral)
///   - target_frozen: true if target is frozen (Evoker Frost Nova applies +25%)
///   - target_marked: true if target has Death Mark (+50%)
///   - crit_mult: critical hit multiplier (1.0 = no crit, 2.5 = shadowstep crit)
pub fn compute_final_damage(
    base_damage: f32,
    class_damage_mult: f32,
    combo_mult: f32,
    skill_mult: f32,
    elemental_mult: f32,
    target_frozen: bool,
    target_marked: bool,
    crit_mult: f32,
) -> f32 {
    let frozen_mult = if target_frozen { 1.25 } else { 1.0 };
    let marked_mult = if target_marked { 1.50 } else { 1.0 };
    base_damage * class_damage_mult * combo_mult * skill_mult * elemental_mult
        * frozen_mult * marked_mult * crit_mult
}

/// Combo system: consecutive hits within 5 seconds (100 ticks) stack a combo.
/// Each combo adds +10% damage, capped at +100% (10 hits).
pub const COMBO_WINDOW_TICKS: u32 = 100;
pub const COMBO_MAX: u32 = 10;

pub fn combo_multiplier(combo_count: u32) -> f32 {
    1.0 + (combo_count.min(COMBO_MAX) as f32) * 0.10
}

// === Attack correlation ===
//
// Pumpkin's `EntityDamageEvent` only gives us the target's entity_id and the
// damage amount — it doesn't tell us WHO dealt the damage. To work around
// this, we listen to `PlayerInteractEntityEvent` (action = Attack) and record
// the mapping (target_entity_id → attacking_player_entity_id, timestamp) in
// a global map. When `EntityDamageEvent` fires shortly after, we look up the
// attacker by target entity_id and apply class/skill modifiers.
//
// Entries expire after 2 ticks (100ms) — long enough for the damage event to
// fire, short enough to avoid stale data when mobs take environmental damage.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug)]
pub struct PendingAttack {
    pub attacker_entity_id: i32,
    pub attacker_player_uuid: uuid::Uuid,
    pub recorded_at_tick: u32,
}

/// Maps target_entity_id → most recent attack by a player.
/// Used to attribute EntityDamageEvent to the correct player.
pub static ATTACK_REGISTRY: LazyLock<Mutex<HashMap<i32, PendingAttack>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record that `attacker_player` attacked `target_entity_id` at `tick`.
pub fn record_attack(target_entity_id: i32, attacker_player_uuid: uuid::Uuid, attacker_entity_id: i32, tick: u32) {
    let mut reg = ATTACK_REGISTRY.lock().unwrap();
    reg.insert(target_entity_id, PendingAttack {
        attacker_entity_id,
        attacker_player_uuid,
        recorded_at_tick: tick,
    });
}

/// Look up the most recent attacker for `target_entity_id`.
/// Returns None if no attack recorded or if it's stale (>2 ticks old).
pub fn lookup_attacker(target_entity_id: i32, current_tick: u32) -> Option<PendingAttack> {
    let reg = ATTACK_REGISTRY.lock().unwrap();
    reg.get(&target_entity_id).filter(|p| current_tick.saturating_sub(p.recorded_at_tick) <= 2).copied()
}

/// Remove entries older than `max_age_ticks`. Called from the tick handler.
pub fn prune_attack_registry(current_tick: u32, max_age_ticks: u32) {
    let mut reg = ATTACK_REGISTRY.lock().unwrap();
    reg.retain(|_, p| current_tick.saturating_sub(p.recorded_at_tick) <= max_age_ticks);
}

// === Mob status effects (applied by skills) ===
//
// Tracked per-mob by entity_id. Pruned when the mob dies or after expiry.

#[derive(Clone, Copy, Debug, Default)]
pub struct MobStatus {
    pub frozen_until_tick: u32,        // Frost Nova: +25% damage taken, no movement
    pub marked_until_tick: u32,        // Death Mark: +50% damage taken
    pub stunned_until_tick: u32,       // Shield Bash: no action
    pub ignited_until_tick: u32,       // Flame Strike / Fireball: DoT
    pub ignited_damage_per_tick: f32,  // DoT amount
    pub silenced_until_tick: u32,      // Arcane Nova: no special abilities (cosmetic for mobs)
}

impl MobStatus {
    pub fn is_frozen(&self, tick: u32) -> bool { tick < self.frozen_until_tick }
    pub fn is_marked(&self, tick: u32) -> bool { tick < self.marked_until_tick }
    pub fn is_stunned(&self, tick: u32) -> bool { tick < self.stunned_until_tick }
    pub fn is_ignited(&self, tick: u32) -> bool { tick < self.ignited_until_tick }
    pub fn is_silenced(&self, tick: u32) -> bool { tick < self.silenced_until_tick }
}

/// Maps mob entity_id → current status effects.
pub static MOB_STATUSES: LazyLock<Mutex<HashMap<i32, MobStatus>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn with_mob_status_mut<F, R>(entity_id: i32, f: F) -> R
where
    F: FnOnce(&mut MobStatus) -> R,
    R: Default,
{
    let mut map = MOB_STATUSES.lock().unwrap();
    let status = map.entry(entity_id).or_insert_with(MobStatus::default);
    f(status)
}

pub fn remove_mob_status(entity_id: i32) {
    MOB_STATUSES.lock().unwrap().remove(&entity_id);
}
