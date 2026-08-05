//! Event handlers — the heart of the plugin.
//!
//! These hook into Pumpkin's event bus to:
//!   - Track player attacks (PlayerInteractEntityEvent) so we can attribute
//!     EntityDamageEvent to the right player
//!   - Modify damage (EntityDamageEvent) — apply class/skill/combo multipliers
//!   - Drop loot on mob death (EntityDeathEvent)
//!   - Grant XP on mob death
//!   - Run the per-tick loop (ServerTickStartEvent) — camera updates, boss
//!     AI, cooldown UI refresh, mob status effect expiry
//!   - Welcome players on join (PlayerJoinEvent)
//!   - Clean up state on disconnect (PlayerLeaveEvent)

use std::sync::Arc;

use pumpkin::plugin::api::events::player::player_interact_entity_event::PlayerInteractEntityEvent;
use pumpkin::plugin::api::events::player::player_join::PlayerJoinEvent;
use pumpkin::plugin::api::events::player::player_leave::PlayerLeaveEvent;
use pumpkin::plugin::api::events::entity::entity_damage::EntityDamageEvent;
use pumpkin::plugin::api::events::entity::entity_death::EntityDeathEvent;
use pumpkin::plugin::api::events::server::server_tick_start::ServerTickStartEvent;
use pumpkin::plugin::{Context, EventHandler, EventPriority};
use pumpkin::server::Server;
use pumpkin_util::text::TextComponent;

use crate::boss;
use crate::camera::CAMERA_MANAGER;
use crate::damage::{self, RpgDamageType};
use crate::player::{self, current_tick, with_state, with_state_mut};
use crate::ui;

// === Attack tracking ===

struct AttackTrackHandler;
impl EventHandler<PlayerInteractEntityEvent> for AttackTrackHandler {
    fn handle<'a>(&'a self, _server: &'a Arc<Server>, event: &'a PlayerInteractEntityEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            use pumpkin_protocol::java::server::play::ActionType;
            if !matches!(event.action, ActionType::Attack) { return; }

            let player = &event.player;
            let player_uuid = player.gameprofile.id;
            let attacker_eid = player.entity_id();
            let target_eid = event.target.entity_id();
            let tick = current_tick();

            damage::record_attack(target_eid, player_uuid, attacker_eid, tick);
        })
    }
}

// === Damage modification ===

struct DamageModHandler;
impl EventHandler<EntityDamageEvent> for DamageModHandler {
    fn handle_blocking<'a>(&'a self, server: &'a Arc<Server>, event: &'a mut EntityDamageEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            let tick = current_tick();
            let target_eid = event.entity_id;

            // Look up the attacker from our registry
            let Some(attack) = damage::lookup_attacker(target_eid, tick) else { return; };
            let attacker_uuid = attack.attacker_player_uuid;

            // Get the player's state (or skip if RPG disabled)
            let rpg_enabled = with_state(attacker_uuid, |s| s.is_enabled());
            if !rpg_enabled { return; }

            // Get the player's class and combo
            let (class, combo, pending_skill_id) = with_state(attacker_uuid, |s| {
                (s.get_class(), s.get_combo(), s.pending_skill_id.load(std::sync::atomic::Ordering::Relaxed))
            });

            // Increment combo
            with_state_mut(attacker_uuid, |s| s.increment_combo(tick));

            // Compute multipliers
            let class_dmg_mult = class.passive_stats().2;
            let combo_mult = damage::combo_multiplier(combo + 1);

            // Skill multiplier + effect application
            let mut skill_mult = 1.0;
            let mut crit_mult = 1.0;
            let mut elemental_mult = 1.0;
            let mut damage_type = class.basic_damage_type();

            if pending_skill_id >= 0 {
                if let Some(skill) = crate::class::SkillDef::by_id(pending_skill_id as usize) {
                    // Apply skill effects
                    match skill.id {
                        0 => { // Shield Bash (Vanguard)
                            skill_mult = 1.5;
                            damage::with_mob_status_mut(target_eid, |s| {
                                s.stunned_until_tick = tick + 40; // 2s stun
                            });
                        }
                        2 => { // Flame Strike (Spellblade)
                            skill_mult = 2.0;
                            damage_type = RpgDamageType::Fire;
                            damage::with_mob_status_mut(target_eid, |s| {
                                s.ignited_until_tick = tick + 80; // 4s burn
                                s.ignited_damage_per_tick = 1.0;
                            });
                        }
                        4 => { // Shadowstep crit (Trickster) — should already be applied
                            crit_mult = 2.5;
                        }
                        5 => { // Fan of Knives (Trickster) — cone AoE
                            skill_mult = 1.2;
                        }
                        6 => { // Fireball (Evoker)
                            skill_mult = 2.5;
                            damage_type = RpgDamageType::Fire;
                        }
                        _ => {
                            skill_mult = 1.0;
                        }
                    }

                    // Consume the pending skill
                    with_state_mut(attacker_uuid, |s| {
                        s.pending_skill_id.store(-1, std::sync::atomic::Ordering::Relaxed);
                    });

                    // Notify the player
                    if let Some(player) = server.get_player_by_uuid(attacker_uuid) {
                        let msg = format!("\u{00a7}b{} triggered! +{:.0}% damage\u{00a7}r",
                            skill.name, (skill_mult - 1.0) * 100.0);
                        ui::show_combat_feedback(&player, msg).await;
                    }
                }
            }

            // Check for Shadowstep crit window
            let shadowstep_until = with_state(attacker_uuid, |s| {
                s.shadowstep_until_tick.load(std::sync::atomic::Ordering::Relaxed)
            });
            if tick < shadowstep_until {
                crit_mult = crit_mult.max(2.5);
            }

            // Check target status effects
            let (frozen, marked) = damage::with_mob_status_mut(target_eid, |s| {
                (s.is_frozen(tick), s.is_marked(tick))
            });

            // Apply the modified damage
            let original = event.damage;
            let final_dmg = damage::compute_final_damage(
                original,
                class_dmg_mult,
                combo_mult,
                skill_mult,
                elemental_mult,
                frozen,
                marked,
                crit_mult,
            );
            event.damage = final_dmg;

            // Show damage feedback to attacker
            if let Some(player) = server.get_player_by_uuid(attacker_uuid) {
                let combo_new = combo + 1;
                let msg = format!(
                    "\u{00a7}e{}{} \u{00a7}7-> \u{00a7}c{:.1} dmg\u{00a7}r \u{00a7}7(combo {}x, {:.1}x mult)\u{00a7}r",
                    damage_type.color_code(),
                    damage_type.display_name(),
                    final_dmg,
                    combo_new,
                    damage::combo_multiplier(combo_new),
                );
                ui::show_combat_feedback(&player, msg).await;
            }

            // If this is a boss, update its HP bar
            if boss::is_boss(target_eid) {
                // We need the boss's current HP. The damage event already has
                // the damage; we'd need to look up the entity to get its HP.
                // For now, just trigger a boss bar update — the actual HP
                // read happens in the tick handler.
                // (Boss HP bar updates happen in the tick loop.)
            }
        })
    }
}

// === Entity death — loot + XP ===

struct EntityDeathHandler;
impl EventHandler<EntityDeathEvent> for EntityDeathHandler {
    fn handle<'a>(&'a self, server: &'a Arc<Server>, event: &'a mut EntityDeathEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            let target_eid = event.entity_id;
            let tick = current_tick();

            // Find the dead entity's position + type by scanning all worlds
            let mut entity_pos: Option<pumpkin_util::math::vector3::Vector3<f64>> = None;
            let mut entity_type_name: Option<String> = None;
            let mut world_ref: Option<Arc<pumpkin::world::World>> = None;

            for world in server.worlds.load().iter() {
                for entity in world.entities.load().iter() {
                    if entity.entity_id() == target_eid {
                        entity_pos = Some(entity.position());
                        entity_type_name = Some(entity.entity_type().name.to_string());
                        world_ref = Some(world.clone());
                        break;
                    }
                }
                if entity_pos.is_some() { break; }
            }

            // Find the attacker (player who killed this mob)
            let attacker = damage::lookup_attacker(target_eid, tick);

            // If this was a boss, clean up boss state
            if boss::is_boss(target_eid) {
                boss::remove_boss(server, target_eid).await;
            }

            // Drop loot
            if let (Some(pos), Some(world), Some(type_name)) = (entity_pos, world_ref.clone(), entity_type_name) {
                let player_level = attacker.map_or(1, |a| {
                    with_state(a.attacker_player_uuid, |s| s.get_level())
                });
                let allow_mythic = boss::is_boss(target_eid);
                crate::loot::drop_loot(&world, pos, &type_name, player_level, allow_mythic).await;
            }

            // Grant XP to the killer
            if let Some(attack) = attacker {
                // XP based on mob type (bosses grant a lot more)
                let xp_amount = if boss::is_boss(target_eid) { 500 } else { 10 };
                let leveled_up = with_state_mut(attack.attacker_player_uuid, |s| s.add_xp(xp_amount));

                if leveled_up {
                    if let Some(player) = server.get_player_by_uuid(attack.attacker_player_uuid) {
                        let new_level = with_state(attack.attacker_player_uuid, |s| s.get_level());
                        let class = with_state(attack.attacker_player_uuid, |s| s.get_class());
                        ui::show_levelup(&player, new_level, class).await;
                    }
                }
            }

            // Clean up mob status
            damage::remove_mob_status(target_eid);
        })
    }
}

// === Tick loop ===

struct TickHandler;
impl EventHandler<ServerTickStartEvent> for TickHandler {
    fn handle<'a>(&'a self, server: &'a Arc<Server>, event: &'a ServerTickStartEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            // Update the global tick counter
            player::CURRENT_TICK.store(event.tick as u32, std::sync::atomic::Ordering::Relaxed);
            let tick = event.tick as u32;

            // 1. Update cameras (every tick for smooth motion)
            CAMERA_MANAGER.tick_all(server).await;

            // 2. Boss AI (every tick; bosses check their own cooldowns)
            boss::tick_all_bosses(server).await;

            // 3. Prune attack registry (every 100 ticks = 5s)
            if tick % 100 == 0 {
                damage::prune_attack_registry(tick, 200);
            }

            // 4. Update action bars (every 10 ticks = 500ms)
            if tick % 10 == 0 {
                for player in server.get_all_players() {
                    let player_uuid = player.gameprofile.id;
                    let state_ref = player::get_or_create(player_uuid);
                    ui::update_action_bar(&player, state_ref.value()).await;
                }
            }

            // 5. Apply ignited DoT to mobs (every 20 ticks = 1s)
            if tick % 20 == 0 {
                // Iterate all mobs with status effects, apply DoT
                let mob_eids: Vec<i32> = damage::MOB_STATUSES.lock().unwrap()
                    .iter().filter_map(|(eid, s)| {
                        if s.is_ignited(tick) { Some(*eid) } else { None }
                    }).collect();

                for eid in mob_eids {
                    // Find the entity and apply damage
                    for world in server.worlds.load().iter() {
                        for entity in world.entities.load().iter() {
                            if entity.entity_id() == eid {
                                let dmg = damage::with_mob_status_mut(eid, |s| {
                                    if s.is_ignited(tick) { s.ignited_damage_per_tick } else { 0.0 }
                                });
                                if dmg > 0.0 {
                                    // Apply fire damage to the entity
                                    if let Some(living) = entity.get_living_entity() {
                                        living.health.fetch_sub(dmg).await;
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        })
    }
}

// === Player join ===

struct JoinHandler;
impl EventHandler<PlayerJoinEvent> for JoinHandler {
    fn handle<'a>(&'a self, _server: &'a Arc<Server>, event: &'a PlayerJoinEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            let player = &event.player;
            let player_uuid = player.gameprofile.id;

            // Initialize RPG state
            let _state = player::get_or_create(player_uuid);

            // Welcome message
            player.send_system_message(&TextComponent::text(
                "\u{00a7}6=== Welcome to Pumpkin-RPG! ===\u{00a7}r\n\
                 \u{00a7}7MC Dungeons style combat, leveling, loot, and bosses.\u{00a7}r\n\
                 \u{00a7}aType /rpg info to see commands.\u{00a7}r"
            )).await;
        })
    }
}

// === Player leave — cleanup ===

struct LeaveHandler;
impl EventHandler<PlayerLeaveEvent> for LeaveHandler {
    fn handle<'a>(&'a self, _server: &'a Arc<Server>, event: &'a PlayerLeaveEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            let player_uuid = event.player.gameprofile.id;

            // Reset camera to first person (cleans up the fake entity)
            // Note: we can't easily do this async here since we don't have
            // mutable access to the player. The camera will be cleaned up
            // when the player's entity is removed from the world.

            // Remove RPG state
            player::remove_player(player_uuid);
        })
    }
}

// === Registration ===

pub async fn register_all(ctx: &Context) -> Result<(), String> {
    // Read-only handlers: blocking=false, use `handle`
    ctx.register_event::<PlayerInteractEntityEvent, _>(
        Arc::new(AttackTrackHandler), EventPriority::Normal, false,
    ).await;
    ctx.register_event::<PlayerJoinEvent, _>(
        Arc::new(JoinHandler), EventPriority::Normal, false,
    ).await;
    ctx.register_event::<PlayerLeaveEvent, _>(
        Arc::new(LeaveHandler), EventPriority::Normal, false,
    ).await;
    ctx.register_event::<ServerTickStartEvent, _>(
        Arc::new(TickHandler), EventPriority::Normal, false,
    ).await;

    // Mutating handlers: blocking=true, use `handle_blocking`
    ctx.register_event::<EntityDamageEvent, _>(
        Arc::new(DamageModHandler), EventPriority::Normal, true,
    ).await;
    ctx.register_event::<EntityDeathEvent, _>(
        Arc::new(EntityDeathHandler), EventPriority::Normal, true,
    ).await;

    ctx.log("Event handlers registered: AttackTrack, DamageMod, EntityDeath, Tick, Join, Leave");
    Ok(())
}
