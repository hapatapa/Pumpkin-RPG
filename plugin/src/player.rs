//! Per-player RPG state: class, level, XP, cooldowns, combo, stats.
//!
//! Stored in a global `DashMap` keyed by player UUID. Created on first
//! interaction (join event or first command). Not persisted across server
//! restarts in v1 — a JSON save/load can be added later via
//! `ctx.get_data_folder()`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

use crate::class::RpgClass;
use crate::damage::MobStatus;

/// Current server tick, updated by the tick handler. Used for cooldown
/// calculations and effect expiry checks. Wraps around every ~3 years of
/// uptime, which is fine.
pub static CURRENT_TICK: AtomicU32 = AtomicU32::new(0);

pub fn current_tick() -> u32 {
    CURRENT_TICK.load(Ordering::Relaxed)
}

/// Approximate tick from wall-clock time (used when we need a tick value
/// outside the tick handler, e.g. in command executors that run between
/// ticks). 50ms per tick.
pub fn approximate_tick() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() / 50) as u32)
        .unwrap_or(0)
}

/// Per-player RPG state. All fields are atomic/mutex-protected so it can be
/// safely shared across threads (Pumpkin dispatches events on a tokio
/// multi-thread runtime).
pub struct PlayerRpgState {
    pub class: Mutex<RpgClass>,
    pub rpg_enabled: AtomicBool,

    // Leveling
    pub level: AtomicI32,
    pub xp: AtomicI32,            // current XP toward next level
    pub skill_points: AtomicI32,  // unspent skill points

    // Combat state
    pub combo_count: AtomicU32,
    pub last_attack_tick: AtomicU32,
    pub pending_skill_id: AtomicI32,    // -1 = none, 0..=11 = skill id

    // Cooldowns: skill_id (usize) → tick when cooldown ends
    pub cooldowns: Mutex<HashMap<usize, u32>>,

    // Active buffs
    pub bulwark_until_tick: AtomicU32,  // Vanguard Bulwark: +50% damage reduction
    pub shadowstep_until_tick: AtomicU32, // Trickster Shadowstep: next attack crits

    // Stats (recomputed on level-up / class change)
    pub max_hp_override: AtomicI32,  // 0 = use vanilla max HP, >0 = use this
}

impl PlayerRpgState {
    pub fn new() -> Self {
        Self {
            class: Mutex::new(RpgClass::Vanguard), // default; player must pick via /class
            rpg_enabled: AtomicBool::new(true),
            level: AtomicI32::new(1),
            xp: AtomicI32::new(0),
            skill_points: AtomicI32::new(0),
            combo_count: AtomicU32::new(0),
            last_attack_tick: AtomicU32::new(0),
            pending_skill_id: AtomicI32::new(-1),
            cooldowns: Mutex::new(HashMap::new()),
            bulwark_until_tick: AtomicU32::new(0),
            shadowstep_until_tick: AtomicU32::new(0),
            max_hp_override: AtomicI32::new(0),
        }
    }

    pub fn get_class(&self) -> RpgClass { *self.class.lock().unwrap() }
    pub fn set_class(&self, c: RpgClass) { *self.class.lock().unwrap() = c; }
    pub fn is_enabled(&self) -> bool { self.rpg_enabled.load(Ordering::Relaxed) }
    pub fn set_enabled(&self, v: bool) { self.rpg_enabled.store(v, Ordering::Relaxed); }
    pub fn get_level(&self) -> i32 { self.level.load(Ordering::Relaxed) }
    pub fn get_xp(&self) -> i32 { self.xp.load(Ordering::Relaxed) }
    pub fn get_skill_points(&self) -> i32 { self.skill_points.load(Ordering::Relaxed) }
    pub fn get_combo(&self) -> u32 { self.combo_count.load(Ordering::Relaxed) }

    // === Cooldowns ===

    /// Returns true if `skill_id` is on cooldown, false otherwise.
    pub fn is_on_cooldown(&self, skill_id: usize) -> bool {
        let cds = self.cooldowns.lock().unwrap();
        cds.get(&skill_id).map_or(false, |&until| current_tick() < until)
    }

    /// Returns remaining cooldown in seconds (0.0 if ready).
    pub fn cooldown_remaining_secs(&self, skill_id: usize) -> f32 {
        let cds = self.cooldowns.lock().unwrap();
        cds.get(&skill_id).map_or(0.0, |&until| {
            let remaining = until.saturating_sub(current_tick());
            remaining as f32 / 20.0
        })
    }

    /// Put `skill_id` on cooldown for `seconds`.
    pub fn start_cooldown(&self, skill_id: usize, seconds: f32) {
        let until = current_tick().saturating_add((seconds * 20.0) as u32);
        self.cooldowns.lock().unwrap().insert(skill_id, until);
    }

    /// Tick down all cooldowns — actually we use absolute tick deadlines so
    /// this is a no-op. Kept for future use if we switch to relative CDs.
    pub fn tick_cooldowns(&self) { /* no-op: cooldowns use absolute tick deadlines */ }

    // === Combo ===

    /// Increment the combo counter if the player attacked recently (within
    /// COMBO_WINDOW_TICKS). Otherwise reset to 1.
    pub fn increment_combo(&self, tick: u32) {
        let last = self.last_attack_tick.load(Ordering::Relaxed);
        if tick.saturating_sub(last) > crate::damage::COMBO_WINDOW_TICKS {
            self.combo_count.store(1, Ordering::Relaxed);
        } else {
            let cur = self.combo_count.load(Ordering::Relaxed);
            if cur < crate::damage::COMBO_MAX {
                self.combo_count.store(cur + 1, Ordering::Relaxed);
            }
        }
        self.last_attack_tick.store(tick, Ordering::Relaxed);
    }

    pub fn combo_multiplier(&self) -> f32 {
        crate::damage::combo_multiplier(self.combo_count.load(Ordering::Relaxed))
    }

    /// Reset combo (e.g. on taking damage, on death, on logout).
    pub fn reset_combo(&self) {
        self.combo_count.store(0, Ordering::Relaxed);
    }

    // === XP & Leveling ===

    /// Add `amount` XP. Returns true if the player leveled up (possibly
    /// multiple times — call `process_levelups` after).
    pub fn add_xp(&self, amount: i32) -> bool {
        let mut leveled_up = false;
        let mut xp = self.xp.load(Ordering::Relaxed);
        let mut level = self.level.load(Ordering::Relaxed);
        xp += amount;
        while xp >= xp_to_next_level(level) {
            xp -= xp_to_next_level(level);
            level += 1;
            leveled_up = true;
        }
        self.xp.store(xp, Ordering::Relaxed);
        self.level.store(level, Ordering::Relaxed);
        if leveled_up {
            // Each level grants 1 skill point
            self.skill_points.fetch_add(level - self.level.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        leveled_up
    }

    pub fn process_levelups(&self) -> Vec<i32> {
        // Returns list of new levels reached (empty if none).
        // Currently the level-up logic in add_xp already handles everything;
        // this method exists for future hooks (broadcasting level-up effects).
        Vec::new()
    }
}

/// XP required to advance from `level` to `level + 1`.
/// Curve: 100 * level^1.5, rounded. So:
///   1→2: 100, 2→3: 283, 3→4: 520, 5→6: 1118, 10→11: 3162, 20→21: 8944
pub fn xp_to_next_level(level: i32) -> i32 {
    let base = 100.0_f32;
    (base * (level as f32).powf(1.5)) as i32
}

/// Total XP needed to reach `level` from level 1 (cumulative).
pub fn total_xp_for_level(level: i32) -> i32 {
    (1..level).map(xp_to_next_level).sum()
}

// === Global player registry ===

pub static PLAYER_STATES: LazyLock<DashMap<uuid::Uuid, PlayerRpgState>> =
    LazyLock::new(DashMap::new);

/// Get or create the RPG state for `player_uuid`.
pub fn get_or_create(player_uuid: uuid::Uuid) -> dashmap::mapref::one::Ref<'static, uuid::Uuid, PlayerRpgState> {
    PLAYER_STATES.entry(player_uuid).or_insert_with(PlayerRpgState::new).downgrade()
}

/// Run a closure with the player's state (read-only).
pub fn with_state<F, R>(player_uuid: uuid::Uuid, f: F) -> R
where
    F: FnOnce(&PlayerRpgState) -> R,
    R: Default,
{
    let entry = PLAYER_STATES.entry(player_uuid).or_insert_with(PlayerRpgState::new);
    f(entry.value())
}

/// Run a closure with the player's state (mutable). Same signature since
/// our state is all-atomic; this is just for clarity.
pub fn with_state_mut<F, R>(player_uuid: uuid::Uuid, f: F) -> R
where
    F: FnOnce(&PlayerRpgState) -> R,
    R: Default,
{
    let entry = PLAYER_STATES.entry(player_uuid).or_insert_with(PlayerRpgState::new);
    f(entry.value())
}

/// Remove a player's state (on disconnect).
pub fn remove_player(player_uuid: uuid::Uuid) {
    PLAYER_STATES.remove(&player_uuid);
}

/// Best-effort cleanup of stale state (called on plugin load).
pub async fn cleanup_stale_state() {
    // In a real implementation this would scan for orphaned entries.
    // For v1 we just clear everything — players get fresh state on next action.
    PLAYER_STATES.clear();
}

// === Mob status helpers (re-export from damage.rs for convenience) ===

pub fn apply_mob_status<F>(entity_id: i32, f: F)
where
    F: FnOnce(&mut MobStatus),
{
    crate::damage::with_mob_status_mut(entity_id, |s| { f(s); });
}
