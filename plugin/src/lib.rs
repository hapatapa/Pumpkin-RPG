//! Pumpkin-RPG plugin — MC Dungeons style combat, leveling, loot, bosses, and
//! custom camera angles for Pumpkin-MC.
//!
//! Architecture:
//!   lib.rs       — plugin entry, registers commands + events, spawns tick loop
//!   class.rs     — 4 classes (Vanguard, Spellblade, Trickster, Evoker) with
//!                  passives, basic-attack modifiers, and active abilities
//!   player.rs    — per-player RPG state (XP, level, class, cooldowns, stats)
//!   damage.rs    — damage type system, class affinities, combo tracking,
//!                  attack correlation (player ↔ target via entity_id)
//!   loot.rs      — loot tables per mob type, item generation, rarity tiers
//!   boss.rs      — boss definitions, phase logic, HP bar management
//!   camera.rs    — 5 camera modes, smooth interpolation, invisible marker
//!                  armor stand, raycast collision
//!   ui.rs        — action bar updates, boss bar, scoreboard
//!   commands.rs  — /class, /stats, /camera, /summon boss, /skillinfo
//!   events.rs    — attack, damage, death, tick, join, respawn handlers

mod boss;
mod camera;
mod class;
mod commands;
mod damage;
mod events;
mod loot;
mod player;
mod ui;

use std::sync::Arc;

use pumpkin::plugin::{Context, Plugin, PluginFuture};

pub struct RpgPlugin;

impl RpgPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Plugin for RpgPlugin {
    fn on_load(&self, ctx: Arc<Context>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move {
            ctx.log("Pumpkin-RPG v0.2 loading...");

            // 1. Register all commands (class picker, stats, camera modes, boss
            //    summon, skill info). Commands are user-facing entry points.
            commands::register_all(&ctx).await?;

            // 2. Register event handlers. These are the heart of the plugin —
            //    they hook into Pumpkin's event bus to modify damage, drop
            //    loot, track kills for XP, manage camera ticks, etc.
            events::register_all(&ctx).await?;

            // 3. Spawn the persistent tick loop. Pumpkin fires
            //    ServerTickStartEvent every tick (50ms), which we use for
            //    cooldowns, camera updates, boss phase checks, and UI refresh.
            //    The handler itself is registered in events::register_all;
            //    nothing extra to spawn here.

            // 4. Clean up any stale per-player state from a previous run
            //    (best-effort — players who were online during a plugin
            //    reload will simply get a fresh state on their next action).
            player::cleanup_stale_state().await;

            ctx.log("Pumpkin-RPG v0.2 loaded! Try: /class, /stats, /camera, /summon boss");
            Ok(())
        })
    }

    fn on_unload(&self, ctx: Arc<Context>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move {
            // Clean up camera entities for any online players so they don't
            // get stuck viewing through an armor stand that no longer exists.
            camera::cleanup_all_cameras(&ctx.server).await;
            // Remove any active boss bars.
            boss::cleanup_all_bosses(&ctx.server).await;
            ctx.log("Pumpkin-RPG v0.2 unloaded.");
            Ok(())
        })
    }
}

// --- Native plugin entry points (Pumpkin-MC native plugin API v2) ---

use std::sync::LazyLock;

#[unsafe(no_mangle)]
pub static METADATA: LazyLock<pumpkin::plugin::PluginMetadata> = LazyLock::new(|| {
    pumpkin::plugin::PluginMetadata {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        authors: env!("CARGO_PKG_AUTHORS")
            .split(',')
            .map(String::from)
            .collect(),
        description: env!("CARGO_PKG_DESCRIPTION").to_string(),
        dependencies: Vec::new(),
        permissions: Vec::new(),
    }
});

#[unsafe(no_mangle)]
pub static PUMPKIN_API_VERSION: u32 = pumpkin::plugin::PLUGIN_API_VERSION;

#[unsafe(no_mangle)]
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(RpgPlugin::new())
}
// CI re-trigger
