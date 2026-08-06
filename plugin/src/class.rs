//! RPG classes — MC Dungeons inspired.
//!
//! Four classes, each with a distinct playstyle:
//!   - Vanguard:  melee bruiser, high HP, shield-based defense, gap-closer
//!   - Spellblade: hybrid melee+magic, enchants weapon with elements, dash strikes
//!   - Trickster: fast rogue, dodge-iframe, backstab crits, poison
//!   - Evoker:    pure caster, AoE spells, summons temporary allies
//!
//! Each class defines:
//!   - Passive: always-on effect (e.g. +20% max HP for Vanguard)
//!   - Basic attack modifier: applied to every melee hit (e.g. +50% damage from behind for Trickster)
//!   - 2 active abilities: triggered via /skill <name>, on cooldowns
//!   - Ultimate: long cooldown, big effect (only 1 implemented per class for v1)

use crate::damage::RpgDamageType;

/// The four playable classes. Pick via `/class <name>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum RpgClass {
    #[default]
    Vanguard,
    Spellblade,
    Trickster,
    Evoker,
}

impl RpgClass {
    /// Parse a class from a user-typed string (case-insensitive).
    /// Accepts full name or short alias.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "vanguard" | "van" | "v" => Some(Self::Vanguard),
            "spellblade" | "spell" | "sb" => Some(Self::Spellblade),
            "trickster" | "trick" | "t" => Some(Self::Trickster),
            "evoker" | "evo" | "e" => Some(Self::Evoker),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Vanguard => "Vanguard",
            Self::Spellblade => "Spellblade",
            Self::Trickster => "Trickster",
            Self::Evoker => "Evoker",
        }
    }

    /// Short description shown in `/class` list.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Vanguard => "Melee tank. High HP, shield defense, gap-closer ability. Frontline bruiser.",
            Self::Spellblade => "Hybrid melee+magic. Enchants weapon with elements, dashes through enemies.",
            Self::Trickster => "Fast rogue. Dodge iframes, backstab crits, poison blades. High mobility.",
            Self::Evoker => "Pure caster. AoE spells, summons temporary allies. Glass cannon.",
        }
    }

    /// The class's signature color (for chat/UI), as a Minecraft color code.
    pub fn color_code(&self) -> &'static str {
        match self {
            Self::Vanguard => "\u{00a7}c",   // red
            Self::Spellblade => "\u{00a7}b", // aqua
            Self::Trickster => "\u{00a7}5",  // purple
            Self::Evoker => "\u{00a7}9",     // blue
        }
    }

    /// Default damage type for the class's basic attacks. Affects which
    /// elemental modifiers apply.
    pub fn basic_damage_type(&self) -> RpgDamageType {
        match self {
            Self::Vanguard => RpgDamageType::Physical,
            Self::Spellblade => RpgDamageType::Arcane,
            Self::Trickster => RpgDamageType::Physical,
            Self::Evoker => RpgDamageType::Arcane,
        }
    }

    /// Passive stat bonuses applied at class selection and on level-up.
    /// Returns (max_hp_bonus, speed_multiplier, damage_multiplier).
    pub fn passive_stats(&self) -> (f32, f32, f32) {
        match self {
            Self::Vanguard => (20.0, 1.0, 1.0),   // +20 HP, normal speed, normal dmg
            Self::Spellblade => (0.0, 1.05, 1.0), // +0 HP, +5% speed
            Self::Trickster => (-5.0, 1.15, 1.0), // -5 HP (glass cannon), +15% speed
            Self::Evoker => (-10.0, 1.0, 1.10),   // -10 HP, +10% spell dmg
        }
    }

    /// List the class's active abilities (excluding the ultimate).
    /// Each entry is (skill_id, name, cooldown_seconds).
    pub fn active_abilities(&self) -> &'static [(usize, &'static str, f32)] {
        match self {
            Self::Vanguard => &[
                (0, "Shield Bash",  6.0),
                (1, "Bulwark",     18.0),
            ],
            Self::Spellblade => &[
                (2, "Flame Strike",  5.0),
                (3, "Frost Dash",    8.0),
            ],
            Self::Trickster => &[
                (4, "Shadowstep",  6.0),
                (5, "Fan of Knives", 10.0),
            ],
            Self::Evoker => &[
                (6, "Fireball",   4.0),
                (7, "Frost Nova", 12.0),
            ],
        }
    }

    /// The class's ultimate ability. Long cooldown, big effect.
    pub fn ultimate(&self) -> (usize, &'static str, f32) {
        match self {
            Self::Vanguard =>  (8, "Earthshatter",  90.0),
            Self::Spellblade => (9, "Arcane Nova",   75.0),
            Self::Trickster =>  (10, "Death Mark",   60.0),
            Self::Evoker =>     (11, "Meteor Storm", 100.0),
        }
    }

    /// All abilities (actives + ultimate) for this class, in skill_id order.
    pub fn all_abilities(&self) -> Vec<(usize, &'static str, f32)> {
        let mut v: Vec<_> = self.active_abilities().iter().copied().collect();
        v.push(self.ultimate());
        v.sort_by_key(|(id, _, _)| *id);
        v
    }
}

// === Skill definitions ===
//
// A skill is identified by a global usize ID (the first column in the tables
// above). Skills are class-locked: a Vanguard cannot cast Fireball. The
// /skill command validates this.
//
// Skill effects are implemented in `events.rs` (in the attack handler and
// tick handler) by checking the player's `pending_skill` field. When a
// skill is activated, we mark it as "pending" — the next melee attack (or
// tick, for self-buffs) consumes it and applies the effect.

#[derive(Clone, Copy, Debug)]
pub struct SkillDef {
    pub id: usize,
    pub name: &'static str,
    pub class: RpgClass,
    pub cooldown_seconds: f32,
    pub description: &'static str,
}

impl SkillDef {
    /// All skills, indexed by id. Keep this in sync with the tables above.
    pub const ALL: [SkillDef; 12] = [
        // Vanguard
        SkillDef { id: 0,  name: "Shield Bash",  class: RpgClass::Vanguard,  cooldown_seconds: 6.0,
            description: "Next melee hit stuns the target for 2s and deals +50% damage." },
        SkillDef { id: 1,  name: "Bulwark",      class: RpgClass::Vanguard,  cooldown_seconds: 18.0,
            description: "Gain 50% damage reduction for 5s." },
        SkillDef { id: 8,  name: "Earthshatter", class: RpgClass::Vanguard,  cooldown_seconds: 90.0,
            description: "Ultimate. Slam the ground, dealing 8 AoE damage + knockback in 6m radius." },
        // Spellblade
        SkillDef { id: 2,  name: "Flame Strike", class: RpgClass::Spellblade, cooldown_seconds: 5.0,
            description: "Next melee hit deals Fire damage (+100%) and ignites the target for 4s." },
        SkillDef { id: 3,  name: "Frost Dash",   class: RpgClass::Spellblade, cooldown_seconds: 8.0,
            description: "Dash 5m forward, leaving a frost trail that slows enemies." },
        SkillDef { id: 9,  name: "Arcane Nova",  class: RpgClass::Spellblade, cooldown_seconds: 75.0,
            description: "Ultimate. Burst of arcane energy in 8m radius, dealing 10 damage + 2s silence." },
        // Trickster
        SkillDef { id: 4,  name: "Shadowstep",   class: RpgClass::Trickster,  cooldown_seconds: 6.0,
            description: "Teleport 8m in look direction. Next attack within 3s is a guaranteed crit (2.5x)." },
        SkillDef { id: 5,  name: "Fan of Knives", class: RpgClass::Trickster,  cooldown_seconds: 10.0,
            description: "Throw 5 knives in a cone, each dealing 4 Physical damage + poison stack." },
        SkillDef { id: 10, name: "Death Mark",   class: RpgClass::Trickster,  cooldown_seconds: 60.0,
            description: "Ultimate. Mark target for 8s. Marked target takes +50% damage from all sources." },
        // Evoker
        SkillDef { id: 6,  name: "Fireball",     class: RpgClass::Evoker,     cooldown_seconds: 4.0,
            description: "Launch a fireball that explodes on impact, dealing 8 Fire damage in 3m radius." },
        SkillDef { id: 7,  name: "Frost Nova",   class: RpgClass::Evoker,     cooldown_seconds: 12.0,
            description: "Freeze all enemies in 5m radius for 3s. Frozen enemies take +25% damage." },
        SkillDef { id: 11, name: "Meteor Storm", class: RpgClass::Evoker,     cooldown_seconds: 100.0,
            description: "Ultimate. Call down 5 meteors over 5s, each dealing 15 Fire damage in 4m radius." },
    ];

    pub fn by_id(id: usize) -> Option<&'static SkillDef> {
        Self::ALL.iter().find(|s| s.id == id)
    }

    pub fn by_name(name: &str) -> Option<&'static SkillDef> {
        let lower = name.to_lowercase().replace(' ', "_");
        Self::ALL.iter().find(|s| s.name.to_lowercase().replace(' ', "_") == lower)
    }
}
