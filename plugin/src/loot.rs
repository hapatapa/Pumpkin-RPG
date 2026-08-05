//! Loot tables and item generation.
//!
//! When a mob dies, we drop RPG items at its location based on:
//!   - The mob's vanilla type (zombie, skeleton, etc.)
//!   - The mob's "rarity roll" (random per kill)
//!   - The killing player's level (higher level = better drops)
//!
//! Rarity tiers (color-coded item names):
//!   Common    (white)   - 60% chance, baseline drops
//!   Rare      (blue)    - 25% chance, +1 enchantment
//!   Epic      (purple)  - 10% chance, +2 enchantments
//!   Legendary (gold)    - 4%  chance, +3 enchantments, custom name
//!   Mythic    (red)     - 1%  chance, +3 enchantments + lore, only from bosses
//!
//! Drop types:
//!   - Gold nuggets (currency for future shop)
//!   - Potions (healing, swiftness, strength)
//!   - Weapons (sword/axe with enchantments, class-flavored)
//!   - Artifacts (consumable items that grant a temporary buff)

use rand::Rng;
use rand::seq::SliceRandom;

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::Enchantment;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin::world::World;

/// Item rarity tiers. Higher = rarer = more enchantments + better name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rarity {
    Common,
    Rare,
    Epic,
    Legendary,
    Mythic,
}

impl Rarity {
    pub fn color_code(&self) -> &'static str {
        match self {
            Self::Common    => "\u{00a7}f", // white
            Self::Rare      => "\u{00a7}b", // aqua
            Self::Epic      => "\u{00a7}5", // purple
            Self::Legendary => "\u{00a7}6", // gold
            Self::Mythic    => "\u{00a7}c", // red
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Common    => "Common",
            Self::Rare      => "Rare",
            Self::Epic      => "Epic",
            Self::Legendary => "Legendary",
            Self::Mythic    => "Mythic",
        }
    }

    /// Number of enchantments to apply.
    pub fn enchantment_count(&self) -> usize {
        match self {
            Self::Common    => 0,
            Self::Rare      => 1,
            Self::Epic      => 2,
            Self::Legendary => 3,
            Self::Mythic    => 3,
        }
    }

    /// Roll a random rarity. Bias toward Common; Mythic only via `allow_mythic=true`.
    pub fn roll(allow_mythic: bool) -> Self {
        let mut rng = rand::thread_rng();
        let roll: f32 = rng.random();
        if allow_mythic && roll < 0.01 {
            Self::Mythic
        } else if roll < 0.05 {
            Self::Legendary
        } else if roll < 0.15 {
            Self::Epic
        } else if roll < 0.40 {
            Self::Rare
        } else {
            Self::Common
        }
    }
}

/// Loot table entry: a single possible drop with weight and min/max quantity.
pub struct LootEntry {
    pub item: &'static Item,
    pub min: u8,
    pub max: u8,
    pub weight: u32,
    pub enchantments: &'static [&'static Enchantment], // pool to roll from if rarity allows
}

/// Vanilla mob type → loot table. We use the entity's type name as the key
/// (lowercased). Unknown mobs fall back to the "default" table.
pub fn loot_table_for_mob(mob_type: &str) -> Vec<LootEntry> {
    match mob_type.to_lowercase().as_str() {
        "zombie" | "husk" | "drowned" => vec![
            LootEntry { item: &Item::GOLD_NUGGET,    min: 1, max: 3,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::ROTTEN_FLESH,   min: 1, max: 2,  weight: 50,  enchantments: &[] },
            LootEntry { item: &Item::IRON_INGOT,     min: 1, max: 1,  weight: 20,  enchantments: &[&Enchantment::SHARPNESS, &Enchantment::KNOCKBACK] },
            LootEntry { item: &Item::STONE_SWORD,    min: 1, max: 1,  weight: 10,  enchantments: &[&Enchantment::SHARPNESS, &Enchantment::KNOCKBACK] },
            LootEntry { item: &Item::IRON_SWORD,     min: 1, max: 1,  weight: 5,   enchantments: &[&Enchantment::SHARPNESS, &Enchantment::FIRE_ASPECT, &Enchantment::KNOCKBACK] },
            LootEntry { item: &Item::POTION,         min: 1, max: 1,  weight: 8,   enchantments: &[] },
        ],
        "skeleton" | "stray" => vec![
            LootEntry { item: &Item::GOLD_NUGGET,    min: 1, max: 3,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::BONE,           min: 1, max: 3,  weight: 80,  enchantments: &[] },
            LootEntry { item: &Item::ARROW,          min: 4, max: 12, weight: 60,  enchantments: &[] },
            LootEntry { item: &Item::BOW,            min: 1, max: 1,  weight: 15,  enchantments: &[&Enchantment::POWER, &Enchantment::PUNCH, &Enchantment::FLAME] },
            LootEntry { item: &Item::IRON_SWORD,     min: 1, max: 1,  weight: 5,   enchantments: &[&Enchantment::SHARPNESS, &Enchantment::SMITE] },
            LootEntry { item: &Item::POTION,         min: 1, max: 1,  weight: 8,   enchantments: &[] },
        ],
        "spider" | "cave_spider" => vec![
            LootEntry { item: &Item::GOLD_NUGGET,    min: 1, max: 2,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::STRING,         min: 1, max: 2,  weight: 80,  enchantments: &[] },
            LootEntry { item: &Item::SPIDER_EYE,     min: 1, max: 1,  weight: 40,  enchantments: &[] },
            LootEntry { item: &Item::IRON_SWORD,     min: 1, max: 1,  weight: 5,   enchantments: &[&Enchantment::SHARPNESS, &Enchantment::BANE_OF_ARTHROPODS] },
        ],
        "creeper" => vec![
            LootEntry { item: &Item::GOLD_NUGGET,    min: 2, max: 5,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::GUNPOWDER,      min: 1, max: 2,  weight: 80,  enchantments: &[] },
            LootEntry { item: &Item::MUSIC_DISC_13,  min: 1, max: 1,  weight: 5,   enchantments: &[] },
            LootEntry { item: &Item::DIAMOND,        min: 1, max: 1,  weight: 3,   enchantments: &[] },
        ],
        "enderman" => vec![
            LootEntry { item: &Item::GOLD_NUGGET,    min: 3, max: 6,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::ENDER_PEARL,    min: 1, max: 2,  weight: 80,  enchantments: &[] },
            LootEntry { item: &Item::ENDER_EYE,      min: 1, max: 1,  weight: 10,  enchantments: &[] },
            LootEntry { item: &Item::DIAMOND,        min: 1, max: 2,  weight: 15,  enchantments: &[] },
        ],
        _ => vec![
            // Default table for any mob not explicitly listed above.
            LootEntry { item: &Item::GOLD_NUGGET,    min: 1, max: 2,  weight: 100, enchantments: &[] },
            LootEntry { item: &Item::IRON_INGOT,     min: 1, max: 1,  weight: 20,  enchantments: &[&Enchantment::SHARPNESS, &Enchantment::KNOCKBACK] },
            LootEntry { item: &Item::POTION,         min: 1, max: 1,  weight: 10,  enchantments: &[] },
        ],
    }
}

/// Generate a single loot drop at `pos` in `world`. Called from the entity
/// death handler.
pub async fn drop_loot(
    world: &std::sync::Arc<World>,
    pos: Vector3<f64>,
    mob_type: &str,
    player_level: i32,
    allow_mythic: bool,
) {
    let table = loot_table_for_mob(mob_type);
    let mut rng = rand::thread_rng();

    // Higher player level → chance for an extra drop
    let extra_drop_chance = (player_level as f32 * 0.02).min(0.5);
    let num_drops = if rng.random::<f32>() < extra_drop_chance { 2 } else { 1 };
    for _ in 0..num_drops {
        // Weighted random selection
        let total_weight: u32 = table.iter().map(|e| e.weight).sum();
        if total_weight == 0 { continue; }
        let mut roll = rng.gen_range(0..total_weight);
        let entry = table.iter().find(|e| {
            roll = roll.saturating_sub(e.weight);
            roll < e.weight
        }).unwrap_or(&table[0]);

        // Roll quantity
        let count = if entry.min == entry.max {
            entry.min
        } else {
            rng.gen_range(entry.min..=entry.max)
        };

        // Roll rarity (better for higher-level players)
        let rarity = roll_rarity_biased_by_level(player_level, allow_mythic);

        // Build the item stack
        let mut stack = ItemStack::new(count, entry.item);
        let display_name = format!("{}{} {}",
            rarity.color_code(),
            rarity.display_name(),
            item_display_name(entry.item),
        );
        stack.set_custom_name(display_name);

        // Apply enchantments based on rarity
        if !entry.enchantments.is_empty() && rarity.enchantment_count() > 0 {
            let ench_pool = entry.enchantments.to_vec();
            let ench_count = rarity.enchantment_count().min(ench_pool.len());
            let chosen: Vec<_> = ench_pool.choose_multiple(&mut rng, ench_count).collect();
            for &ench in chosen {
                let level = rng.gen_range(1..=max_enchant_level(ench, rarity));
                stack.add_enchantment(ench, level as u16);
            }
        }

        // Drop it
        let block_pos = BlockPos(Vector3::new(
            pos.x.floor() as i32,
            pos.y.floor() as i32,
            pos.z.floor() as i32,
        ));
        world.drop_stack(&block_pos, stack).await;
    }
}

/// Roll a rarity, biased toward higher tiers for higher-level players.
/// At level 1: same as Rarity::roll().
/// At level 50+: ~50% chance to bump up one tier.
fn roll_rarity_biased_by_level(level: i32, allow_mythic: bool) -> Rarity {
    let base = Rarity::roll(allow_mythic);
    let mut rng = rand::thread_rng();
    let bump_chance = (level as f32 * 0.01).min(0.5);
    if rng.random::<f32>() < bump_chance {
        match base {
            Rarity::Common    => Rarity::Rare,
            Rarity::Rare      => Rarity::Epic,
            Rarity::Epic      => Rarity::Legendary,
            Rarity::Legendary => if allow_mythic { Rarity::Mythic } else { Rarity::Legendary },
            Rarity::Mythic    => Rarity::Mythic,
        }
    } else {
        base
    }
}

/// Human-readable name for an item, used in the loot display name.
fn item_display_name(item: &Item) -> String {
    // Item::registry_key is the registry name (e.g. "minecraft:iron_sword").
    // Strip the namespace and prettify the rest.
    let raw = item.registry_key;
    let name_part = raw.rsplit_once(':').map_or(raw, |(_, n)| n);
    name_part.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Max enchantment level by rarity (better rarity = higher enchants).
fn max_enchant_level(_ench: &Enchantment, rarity: Rarity) -> u8 {
    match rarity {
        Rarity::Common    => 1,
        Rarity::Rare      => 2,
        Rarity::Epic      => 3,
        Rarity::Legendary => 4,
        Rarity::Mythic    => 5,
    }
}
