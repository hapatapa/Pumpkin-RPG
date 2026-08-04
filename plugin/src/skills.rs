/// Damage types with an advantage cycle:
/// Physical -> Fire -> Magic -> Holy -> Dark -> Ice -> Physical (1.5x)
/// Disadvantage is 0.75x.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RpgDamageType {
    Physical,
    Fire,
    Magic,
    Holy,
    Dark,
    Ice,
}

impl RpgDamageType {
    pub fn advantage_against(&self, other: &Self) -> f32 {
        let order = [Self::Physical, Self::Fire, Self::Magic, Self::Holy, Self::Dark, Self::Ice];
        let self_idx = order.iter().position(|&x| x == *self).unwrap_or(0);
        let other_idx = order.iter().position(|&x| x == *other).unwrap_or(0);
        // Next in cycle = advantage
        if (self_idx + 1) % 6 == other_idx {
            1.5
        } else if (other_idx + 1) % 6 == self_idx {
            0.75
        } else {
            1.0
        }
    }

    pub fn color_code(&self) -> &str {
        match self {
            Self::Physical => "\u{00a7}7", // gray
            Self::Fire => "\u{00a7}c",     // red
            Self::Magic => "\u{00a7}5",    // purple
            Self::Holy => "\u{00a7}e",     // yellow
            Self::Dark => "\u{00a7}8",     // dark gray
            Self::Ice => "\u{00a7}b",      // aqua
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "physical" => Some(Self::Physical),
            "fire" => Some(Self::Fire),
            "magic" => Some(Self::Magic),
            "holy" => Some(Self::Holy),
            "dark" => Some(Self::Dark),
            "ice" => Some(Self::Ice),
            _ => None,
        }
    }
}

pub struct Skill {
    pub id: usize,
    pub name: &'static str,
    pub damage_multiplier: f32,
    pub damage_type: RpgDamageType,
    pub cooldown_ticks: u32,
    pub particle_id: i32,
    pub particle_count: i32,
    pub aoe_radius: f32,
    pub knockback_multiplier: f32,
}

impl Skill {
    pub const ALL: [Self; 8] = [
        Self {
            id: 0,
            name: "Power Strike",
            damage_multiplier: 1.8,
            damage_type: RpgDamageType::Physical,
            cooldown_ticks: 60,
            particle_id: 7,    // instant_effect
            particle_count: 15,
            aoe_radius: 0.0,
            knockback_multiplier: 1.5,
        },
        Self {
            id: 1,
            name: "Flame Slash",
            damage_multiplier: 2.0,
            damage_type: RpgDamageType::Fire,
            cooldown_ticks: 80,
            particle_id: 12,   // flame
            particle_count: 20,
            aoe_radius: 3.0,
            knockback_multiplier: 0.8,
        },
        Self {
            id: 2,
            name: "Arcane Blast",
            damage_multiplier: 2.5,
            damage_type: RpgDamageType::Magic,
            cooldown_ticks: 100,
            particle_id: 14,   // enchant
            particle_count: 25,
            aoe_radius: 4.0,
            knockback_multiplier: 0.5,
        },
        Self {
            id: 3,
            name: "Healing Light",
            damage_multiplier: 0.0,
            damage_type: RpgDamageType::Holy,
            cooldown_ticks: 120,
            particle_id: 26,   // heart
            particle_count: 10,
            aoe_radius: 0.0,
            knockback_multiplier: 0.0,
        },
        Self {
            id: 4,
            name: "Shadow Strike",
            damage_multiplier: 2.2,
            damage_type: RpgDamageType::Dark,
            cooldown_ticks: 50,
            particle_id: 15,   // instant_effect (witch)
            particle_count: 12,
            aoe_radius: 0.0,
            knockback_multiplier: 0.3,
        },
        Self {
            id: 5,
            name: "Frost Nova",
            damage_multiplier: 1.6,
            damage_type: RpgDamageType::Ice,
            cooldown_ticks: 90,
            particle_id: 20,   // snowflake
            particle_count: 30,
            aoe_radius: 5.0,
            knockback_multiplier: 1.2,
        },
        Self {
            id: 6,
            name: "Whirlwind",
            damage_multiplier: 1.4,
            damage_type: RpgDamageType::Physical,
            cooldown_ticks: 70,
            particle_id: 3,    // explosion
            particle_count: 20,
            aoe_radius: 4.0,
            knockback_multiplier: 2.0,
        },
        Self {
            id: 7,
            name: "Divine Smite",
            damage_multiplier: 3.0,
            damage_type: RpgDamageType::Holy,
            cooldown_ticks: 200,
            particle_id: 9,    // cloud
            particle_count: 40,
            aoe_radius: 3.5,
            knockback_multiplier: 1.0,
        },
    ];

    pub fn by_name(name: &str) -> Option<&'static Self> {
        Self::ALL.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}
