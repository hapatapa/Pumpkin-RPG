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
use pumpkin::plugin::api::events::entity::entity_death::EntityDeathEvent;
use pumpkin::plugin::api::events::server::server_tick_start::ServerTickStartEvent;
use pumpkin::plugin::{Context, EventHandler, EventPriority, Cancellable};
use pumpkin::server::Server;
use pumpkin_util::text::TextComponent;
use pumpkin_data::attributes::Attributes;

use crate::boss;
use crate::camera::CAMERA_MANAGER;
use crate::damage::{self, RpgDamageType};
use crate::player::{self, current_tick, with_state, with_state_mut};
use crate::ui;

// === Attack handler (blocking) ===
//
// Pumpkin has a design issue: if PVP is disabled in the server config, ALL
// entity attacks are blocked (not just player-vs-player). The code at
// play.rs:1849 does `if !config.enabled { return; }` before calling
// player.attack(), so mobs never take damage and EntityDamageEvent never
// fires.
//
// Workaround: register this handler as blocking, directly apply RPG damage
// to the mob, and cancel the event so the vanilla 'after' block (including
// the PVP check) never runs. This bypasses the PVP gate entirely.

struct AttackHandler;
impl EventHandler<PlayerInteractEntityEvent> for AttackHandler {
    fn handle_blocking<'a>(&'a self, server: &'a Arc<Server>, event: &'a mut PlayerInteractEntityEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            use pumpkin_protocol::java::server::play::ActionType;
            if !matches!(event.action, ActionType::Attack) { return; }

            // Don't process attacks on other players (let vanilla handle PVP)
            let target = &event.target;
            let target_entity = target.get_entity();
            let target_eid = target_entity.entity_id;

            // Skip if target is a player — vanilla handles PVP
            if target_entity.entity_type == pumpkin_data::entity::EntityType::PLAYER {
                return;
            }

            let player = &event.player;
            let player_uuid = player.gameprofile.id;
            let attacker_eid = player.entity_id();
            let tick = current_tick();

            // Record the attack for death/XP attribution
            damage::record_attack(target_eid, player_uuid, attacker_eid, tick);

            // Check if RPG is enabled for this player
            let rpg_enabled = with_state(player_uuid, |s| s.is_enabled());
            if !rpg_enabled {
                // RPG disabled — let vanilla handle the attack (don't cancel)
                return;
            }

            // Get player state
            let (class, combo, pending_skill_id, level) = with_state(player_uuid, |s| {
                (s.get_class(), s.get_combo(), s.pending_skill_id.load(std::sync::atomic::Ordering::Relaxed), s.get_level())
            });

            // Increment combo
            with_state_mut(player_uuid, |s| s.increment_combo(tick));

            // Compute base damage from player's attack attribute
            let base_damage = player.living_entity.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
            // If base damage is 0 (fist with no attribute), use a minimum of 1.0
            let base_damage = base_damage.max(1.0);

            // Compute multipliers
            let class_dmg_mult = class.passive_stats().2;
            let combo_mult = damage::combo_multiplier(combo + 1);

            // Skill multiplier + effect application
            let mut skill_mult: f32 = 1.0;
            let mut crit_mult: f32 = 1.0;
            let mut damage_type = class.basic_damage_type();

            if pending_skill_id >= 0 {
                if let Some(skill) = crate::class::SkillDef::by_id(pending_skill_id as usize) {
                    match skill.id {
                        0 => { // Shield Bash (Vanguard)
                            skill_mult = 1.5;
                            damage::with_mob_status_mut(target_eid, |s| {
                                s.stunned_until_tick = tick + 40;
                            });
                        }
                        2 => { // Flame Strike (Spellblade)
                            skill_mult = 2.0;
                            damage_type = RpgDamageType::Fire;
                            damage::with_mob_status_mut(target_eid, |s| {
                                s.ignited_until_tick = tick + 80;
                                s.ignited_damage_per_tick = 1.0;
                            });
                        }
                        4 => { // Shadowstep crit (Trickster)
                            crit_mult = 2.5;
                        }
                        5 => { // Fan of Knives (Trickster)
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
                    with_state_mut(player_uuid, |s| {
                        s.pending_skill_id.store(-1, std::sync::atomic::Ordering::Relaxed);
                    });

                    // Notify the player
                    let msg = format!("\u{00a7}b{} triggered! +{:.0}% damage\u{00a7}r",
                        skill.name, (skill_mult - 1.0) * 100.0);
                    ui::show_combat_feedback(player, msg).await;
                }
            }

            // Check for Shadowstep crit window
            let shadowstep_until = with_state(player_uuid, |s| {
                s.shadowstep_until_tick.load(std::sync::atomic::Ordering::Relaxed)
            });
            if tick < shadowstep_until {
                crit_mult = crit_mult.max(2.5_f32);
            }

            // Check target status effects
            let (frozen, marked) = damage::with_mob_status_mut(target_eid, |s| {
                (s.is_frozen(tick), s.is_marked(tick))
            });

            // Compute final damage
            let final_dmg = damage::compute_final_damage(
                base_damage,
                class_dmg_mult,
                combo_mult,
                skill_mult,
                1.0, // elemental_mult (not used yet)
                frozen,
                marked,
                crit_mult,
            );

            // Apply damage directly to the mob's health
            if let Some(living) = target.get_living_entity() {
                let current_hp = living.health.load();
                let new_hp = (current_hp - final_dmg).max(0.0);
                living.set_health(new_hp);

                // If HP reached 0, the mob will die naturally via Pumpkin's
                // death check on the next tick. EntityDeathEvent will fire.
            }

            // Show damage feedback to attacker
            let combo_new = combo + 1;
            let msg = format!(
                "\u{00a7}e{}{} \u{00a7}7-> \u{00a7}c{:.1} dmg\u{00a7}r \u{00a7}7(combo {}x, {:.1}x mult)\u{00a7}r",
                damage_type.color_code(),
                damage_type.display_name(),
                final_dmg,
                combo_new,
                damage::combo_multiplier(combo_new),
            );
            ui::show_combat_feedback(player, msg).await;

            // Play attack sound + particles at target position
            let target_pos = target_entity.pos.load();
            use pumpkin_data::sound::{Sound, SoundCategory};
            use pumpkin_data::particle::Particle;
            player.world().play_sound(Sound::EntityPlayerAttackStrong, SoundCategory::Players, &target_pos);
            player.world().spawn_particle(target_pos, pumpkin_util::math::vector3::Vector3::new(0.5, 0.5, 0.5), 0.2, 5, Particle::DamageIndicator);

            // Cancel the event so vanilla's 'after' block (with the PVP check)
            // doesn't run. This prevents double damage and bypasses the PVP
            // gate that would otherwise block all attacks.
            event.set_cancelled(true);
        })
    }
}

// === Entity death — loot + XP ===

struct EntityDeathHandler;
impl EventHandler<EntityDeathEvent> for EntityDeathHandler {
    fn handle_blocking<'a>(&'a self, server: &'a Arc<Server>, event: &'a mut EntityDeathEvent) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            let target_eid = event.entity_id;
            let tick = current_tick();

            // Find the dead entity's position + type by scanning all worlds
            let mut entity_pos: Option<pumpkin_util::math::vector3::Vector3<f64>> = None;
            let mut entity_type_name: Option<String> = None;
            let mut world_ref: Option<Arc<pumpkin::world::World>> = None;

            for world in server.worlds.load().iter() {
                for entity in world.entities.load().iter() {
                    if entity.get_entity().entity_id == target_eid {
                        entity_pos = Some(entity.get_entity().pos.load());
                        entity_type_name = Some(entity.get_entity().entity_type.resource_name.to_string());
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
                            if entity.get_entity().entity_id == eid {
                                let dmg = damage::with_mob_status_mut(eid, |s| {
                                    if s.is_ignited(tick) { s.ignited_damage_per_tick } else { 0.0 }
                                });
                                if dmg > 0.0 {
                                    // Apply fire damage to the entity
                                    if let Some(living) = entity.get_living_entity() {
                                        let cur = living.health.load();
                                        living.set_health(cur - dmg);
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
    // AttackHandler is blocking because it cancels the event (to bypass
    // Pumpkin's PVP gate that blocks all entity attacks when PVP is off)
    // and directly applies damage to the mob.
    ctx.register_event::<PlayerInteractEntityEvent, _>(
        Arc::new(AttackHandler), EventPriority::Normal, true,
    ).await;
    ctx.register_event::<EntityDeathEvent, _>(
        Arc::new(EntityDeathHandler), EventPriority::Normal, true,
    ).await;

    ctx.log("Event handlers registered: Attack, EntityDeath, Tick, Join, Leave");
    Ok(())
}
