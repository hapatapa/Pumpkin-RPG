use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering}};

use skills::RpgDamageType;
use dashmap::DashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RpgClass {
    Warrior,
    Mage,
    Rogue,
    Paladin,
}

impl RpgClass {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "warrior" => Some(Self::Warrior),
            "mage" => Some(Self::Mage),
            "rogue" => Some(Self::Rogue),
            "paladin" => Some(Self::Paladin),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Warrior => "Warrior",
            Self::Mage => "Mage",
            Self::Rogue => "Rogue",
            Self::Paladin => "Paladin",
        }
    }

    /// Damage multiplier when using a skill of this damage type.
    pub fn damage_multiplier_for(&self, dmg_type: &RpgDamageType) -> f32 {
        match self {
            Self::Warrior => match dmg_type {
                RpgDamageType::Physical => 1.3,
                RpgDamageType::Fire => 1.0,
                RpgDamageType::Ice => 1.1,
                _ => 0.9,
            },
            Self::Mage => match dmg_type {
                RpgDamageType::Magic => 1.4,
                RpgDamageType::Fire => 1.2,
                RpgDamageType::Holy => 1.1,
                _ => 0.8,
            },
            Self::Rogue => match dmg_type {
                RpgDamageType::Dark => 1.3,
                RpgDamageType::Physical => 1.2,
                RpgDamageType::Ice => 1.0,
                _ => 0.9,
            },
            Self::Paladin => match dmg_type {
                RpgDamageType::Holy => 1.4,
                RpgDamageType::Fire => 1.1,
                RpgDamageType::Physical => 1.1,
                _ => 0.9,
            },
        }
    }

    /// Damage resistance against a given damage type.
    pub fn resistance_for(&self, dmg_type: &RpgDamageType) -> f32 {
        // Returns a multiplier less than 1.0 = damage reduction
        match self {
            Self::Warrior => match dmg_type {
                RpgDamageType::Physical => 0.7,
                RpgDamageType::Fire => 0.9,
                _ => 0.85,
            },
            Self::Mage => match dmg_type {
                RpgDamageType::Magic => 0.6,
                RpgDamageType::Holy => 0.8,
                _ => 0.95,
            },
            Self::Rogue => match dmg_type {
                RpgDamageType::Dark => 0.7,
                RpgDamageType::Physical => 0.85,
                _ => 0.9,
            },
            Self::Paladin => match dmg_type {
                RpgDamageType::Holy => 0.5,
                RpgDamageType::Fire => 0.75,
                RpgDamageType::Dark => 0.7,
                _ => 0.85,
            },
        }
    }
}

/// Per-player RPG combat state, stored in a global map keyed by entity_id.
pub struct RpgCombatState {
    pub enabled: AtomicBool,
    pub rpg_class: Mutex<RpgClass>,
    pub cooldowns: Mutex<HashMap<&'static str, u32>>, // skill_name -> remaining ticks
    pub combo_count: AtomicI32,
    pub last_attack_tick: AtomicU32,
    pub last_used_skill: AtomicI32, // 0 = none, 1-8 = skill index+1
}

impl RpgCombatState {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            rpg_class: Mutex::new(RpgClass::Warrior),
            cooldowns: Mutex::new(HashMap::new()),
            combo_count: AtomicI32::new(0),
            last_attack_tick: AtomicU32::new(0),
            last_used_skill: AtomicI32::new(0),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, val: bool) {
        self.enabled.store(val, Ordering::Relaxed);
    }

    pub fn get_class(&self) -> RpgClass {
        *self.rpg_class.lock().unwrap()
    }

    pub fn set_class(&self, cls: RpgClass) {
        *self.rpg_class.lock().unwrap() = cls;
    }

    pub fn use_skill(&self, skill_index: usize) -> bool {
        let name = crate::skills::Skill::ALL.get(skill_index).map(|s| s.name);
        let Some(name) = name else { return false };
        let mut cds = self.cooldowns.lock().unwrap();
        if let Some(remaining) = cds.get(name) {
            if *remaining > 0 { return false; }
        }
        let skill = &crate::skills::Skill::ALL[skill_index];
        cds.insert(name, skill.cooldown_ticks);
        self.last_used_skill.store((skill_index + 1) as i32, Ordering::Relaxed);
        true
    }

    /// Tick down all cooldowns by 1. Called every server tick.
    pub fn tick_cooldowns(&self) {
        let mut cds = self.cooldowns.lock().unwrap();
        for remaining in cds.values_mut() {
            *remaining = remaining.saturating_sub(1);
        }
    }

    pub fn increment_combo(&self, current_tick: u32) {
        let last = self.last_attack_tick.load(Ordering::Relaxed);
        if current_tick.saturating_sub(last) > 100 {
            self.combo_count.store(1, Ordering::Relaxed);
        } else {
            let current = self.combo_count.load(Ordering::Relaxed);
            if current < 10 {
                self.combo_count.store(current + 1, Ordering::Relaxed);
            }
        }
        self.last_attack_tick.store(current_tick, Ordering::Relaxed);
    }

    /// Returns combo multiplier: 1.0 + 0.1 * combo_count (max 2.0 at 10 hits)
    pub fn combo_multiplier(&self) -> f32 {
        let count = self.combo_count.load(Ordering::Relaxed);
        1.0 + f32::from(count) * 0.1
    }

    /// Total damage modifier combining class affinity and combo.
    pub fn total_damage_modifier(&self, skill_damage_type: &RpgDamageType) -> f32 {
        let class_mod = self.get_class().damage_multiplier_for(skill_damage_type);
        class_mod * self.combo_multiplier()
    }

    pub fn get_remaining_cooldown(&self, skill_name: &str) -> u32 {
        self.cooldowns.lock().unwrap().get(skill_name).copied().unwrap_or(0)
    }
}

/// Global state: maps player entity_id -> RpgCombatState
pub static PLAYER_STATES: LazyLock<DashMap<i32, RpgCombatState>> =
    LazyLock::new(DashMap::new);

// Helper functions that work with DashMap directly.
pub fn with_player_state<F, R>(entity_id: i32, f: F) -> R
where
    F: FnOnce(&RpgCombatState) -> R,
{
    let entry = PLAYER_STATES.entry(entity_id).or_insert_with(RpgCombatState::new);
    f(entry.value())
}

pub fn with_player_state_mut<F, R>(entity_id: i32, f: F) -> R
where
    F: FnOnce(&RpgCombatState) -> R,
{
    let entry = PLAYER_STATES.entry(entity_id).or_insert_with(RpgCombatState::new);
    f(entry.value())
}
