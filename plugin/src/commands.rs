use std::sync::Arc;

use pumpkin::command::args::ConsumedArgs;
use pumpkin::command::dispatcher::CommandError::InvalidRequirement;
use pumpkin::command::tree::builder::literal;
use pumpkin::command::tree::CommandTree;
use pumpkin::command::{CommandExecutor, CommandResult, CommandSender};
use pumpkin::net::ClientPlatform;
use pumpkin::plugin::api::events::player::PlayerInteractEntityEvent;
use pumpkin::plugin::api::events::server::server_tick_start::ServerTickStartEvent;
use pumpkin::plugin::api::{EventPriority, Payload};
use pumpkin::plugin::EventHandler;
use pumpkin_protocol::java::client::play::{CSetCamera, CRemoveEntities};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;

use crate::camera::{self, CameraMode};
use crate::combat::{self, RpgClass, PLAYER_STATES};
use crate::skills::Skill;

// ===== EVENT HANDLERS =====

struct AttackHandler;

impl EventHandler<PlayerInteractEntityEvent> for AttackHandler {
    fn handle<'a>(
        &'a self,
        _server: &'a Arc<pumpkin::server::Server>,
        event: &'a PlayerInteractEntityEvent,
    ) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            use pumpkin_protocol::java::server::play::ActionType;

            if !matches!(event.action, ActionType::Attack) {
                return;
            }

            let player = &event.player;
            let attacker_id = player.entity_id();

            // Check if RPG is enabled for this player
            let rpg_enabled = combat::with_player_state(attacker_id, |state| state.is_enabled());
            if !rpg_enabled {
                return;
            }

            // Approximate server tick from time
            let tick = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.as_millis() / 50) as u32)
                .unwrap_or(0);

            // Increment combo
            combat::with_player_state_mut(attacker_id, |state| {
                state.increment_combo(tick);
            });

            // Check if a skill was used recently (within last 5 ticks)
            let skill_idx = combat::with_player_state(attacker_id, |state| {
                state.last_used_skill.load(std::sync::atomic::Ordering::Relaxed)
            });

            if skill_idx > 0 {
                let idx = (skill_idx - 1) as usize;
                if let Some(skill) = Skill::ALL.get(idx) {
                    let dmg_type = skill.damage_type;
                    let total_mod = combat::with_player_state(attacker_id, |state| {
                        state.total_damage_modifier(&dmg_type)
                    });

                    let combo = combat::with_player_state(attacker_id, |state| {
                        state.combo_count.load(std::sync::atomic::Ordering::Relaxed)
                    });

                    let cls = combat::with_player_state(attacker_id, |state| state.get_class());
                    let cls_name = cls.display_name();

                    let _ = player
                        .send_system_message(&TextComponent::text(format!(
                            "{}{} [{}] {}x combo ({}x dmg)",
                            dmg_type.color_code(),
                            skill.name,
                            cls_name,
                            combo,
                            total_mod,
                        )))
                        .await;

                    // Reset skill after applying
                    combat::with_player_state_mut(attacker_id, |state| {
                        state
                            .last_used_skill
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                    });
                }
            }
        })
    }
}

struct TickHandler;

impl EventHandler<ServerTickStartEvent> for TickHandler {
    fn handle<'a>(
        &'a self,
        server: &'a Arc<pumpkin::server::Server>,
        _event: &'a ServerTickStartEvent,
    ) -> pumpkin::plugin::BoxFuture<'a, ()> {
        Box::pin(async move {
            // Tick cooldowns for all tracked players
            for entry in PLAYER_STATES.iter() {
                entry.value().tick_cooldowns();
            }

            // Update camera positions for all players in custom camera modes
            for world in server.worlds.load().iter() {
                for player in world.players.load().iter() {
                    let player_id = player.entity_id();
                    let cam_mgr = &camera::CAMERA_MANAGER;

                    if let Some(cam_state) = cam_mgr.get_camera(player_id) {
                        if matches!(cam_state.mode, CameraMode::FirstPerson) {
                            continue;
                        }

                        let player_pos = player.position();
                        let player_yaw = player.living_entity.entity.yaw.load();

                        let (cam_pos, cam_yaw) =
                            camera::calculate_camera_pos(player_pos, player_yaw, &cam_state.mode);

                        let teleport_packet =
                            camera::build_teleport_packet(&cam_state, cam_pos, cam_yaw, 0.0);

                        player
                            .client
                            .send_packet_now(&teleport_packet)
                            .await;
                    }
                }
            }
        })
    }
}

// ===== COMMAND EXECUTORS =====

struct SkillListExecutor;
impl CommandExecutor for SkillListExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let mut msg = String::from("\u{00a7}6=== RPG Skills ===\u{00a7}r\n");
            for skill in &Skill::ALL {
                let cd_secs = skill.cooldown_ticks as f64 / 20.0;
                msg.push_str(&format!(
                    "{}{}\u{00a7}r - {}x damage, {:.1}s CD, {}\n",
                    skill.damage_type.color_code(),
                    skill.name,
                    skill.damage_multiplier,
                    cd_secs,
                    match skill.aoe_radius > 0.0 {
                        true => format!("AOE {:.0}m", skill.aoe_radius),
                        false => "Single target".to_string(),
                    },
                ));
            }
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

struct SkillUseExecutor {
    skill_name: String,
}
impl CommandExecutor for SkillUseExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let entity_id = player.entity_id();
            let rpg_enabled = combat::with_player_state(entity_id, |s| s.is_enabled());
            if !rpg_enabled {
                sender
                    .send_message(TextComponent::text(
                        "\u{00a7}cRPG is disabled! Use /rpgclass toggle\u{00a7}r",
                    ))
                    .await;
                return Ok(0);
            }

            let Some(skill) = Skill::by_name(&self.skill_name) else {
                sender
                    .send_message(TextComponent::text(format!(
                        "\u{00a7}cUnknown skill: {}\u{00a7}r",
                        self.skill_name
                    )))
                    .await;
                return Ok(0);
            };

            let used = combat::with_player_state_mut(entity_id, |state| state.use_skill(skill.id));

            if !used {
                let remaining = combat::with_player_state(entity_id, |state| {
                    state.get_remaining_cooldown(skill.name)
                });
                sender
                    .send_message(TextComponent::text(format!(
                        "\u{00a7}c{} is on cooldown! {:.1}s remaining\u{00a7}r",
                        skill.name,
                        remaining as f64 / 20.0,
                    )))
                    .await;
                return Ok(0);
            }

            sender
                .send_message(TextComponent::text(format!(
                    "{}Used {}!\u{00a7}r (next attack deals {}x {} damage)",
                    skill.damage_type.color_code(),
                    skill.name,
                    skill.damage_multiplier,
                    format!("{:?}", skill.damage_type),
                )))
                .await;
            Ok(1)
        })
    }
}

struct CameraListExecutor;
impl CommandExecutor for CameraListExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let modes = [
                ("firstperson", "First Person"),
                ("overshoulder", "Over Shoulder"),
                ("topdown", "Top Down"),
                ("cinematic", "Cinematic"),
                ("combatcam", "Combat Cam"),
            ];
            let mut msg = String::from("\u{00a7}6=== Camera Modes ===\u{00a7}r\n");
            for (cmd, display) in &modes {
                msg.push_str(&format!(
                    "  \u{00a7}a/camera {}\u{00a7}r - {}\n",
                    cmd, display
                ));
            }
            msg.push_str("  \u{00a7}a/camera reset\u{00a7}r - Reset to default\n");
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

struct CameraSetExecutor {
    mode_name: String,
}
impl CommandExecutor for CameraSetExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let Some(mode) = CameraMode::from_name(&self.mode_name) else {
                sender
                    .send_message(TextComponent::text(format!(
                        "\u{00a7}cUnknown camera mode: {}\u{00a7}r",
                        self.mode_name
                    )))
                    .await;
                return Ok(0);
            };

            let entity_id = player.entity_id();
            let cam_mgr = &camera::CAMERA_MANAGER;

            // If FirstPerson, just reset
            if matches!(mode, CameraMode::FirstPerson) {
                if let Some(old_cam) = cam_mgr.remove_camera(entity_id) {
                    let ids = [VarInt(old_cam.fake_entity_id)];
                    player.client.send_packet_now(&CRemoveEntities::new(&ids)).await;
                }
                player
                    .client
                    .send_packet_now(&CSetCamera::new(VarInt(entity_id)))
                    .await;
                player.camera_target_id.store(None);
                sender
                    .send_message(TextComponent::text(
                        "\u{00a7}aCamera reset to First Person\u{00a7}r",
                    ))
                    .await;
                return Ok(1);
            }

            // Remove old camera if any
            if let Some(old_cam) = cam_mgr.remove_camera(entity_id) {
                let ids = [VarInt(old_cam.fake_entity_id)];
                player
                    .client
                    .send_packet_now(&CRemoveEntities::new(&ids))
                    .await;
            }

            // Spawn new camera entity
            let fake_entity_id = cam_mgr.set_camera_mode(entity_id, mode);
            let cam_state = cam_mgr.get_camera(entity_id).unwrap();

            let player_pos = player.position();
            let player_yaw = player.living_entity.entity.yaw.load();
            let (cam_pos, cam_yaw) = camera::calculate_camera_pos(player_pos, player_yaw, &mode);

            // Spawn invisible armor stand
            let spawn_packet = camera::build_spawn_packet(&cam_state, cam_pos, cam_yaw);
            player.client.send_packet_now(&spawn_packet).await;

            // Set camera to the fake entity
            player
                .client
                .send_packet_now(&CSetCamera::new(VarInt(fake_entity_id)))
                .await;

            player.camera_target_id.store(Some(fake_entity_id));

            sender
                .send_message(TextComponent::text(format!(
                    "\u{00a7}aCamera set to {}\u{00a7}r",
                    mode.display_name()
                )))
                .await;
            Ok(1)
        })
    }
}

struct CameraResetExecutor;
impl CommandExecutor for CameraResetExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let entity_id = player.entity_id();
            let cam_mgr = &camera::CAMERA_MANAGER;

            if let Some(old_cam) = cam_mgr.remove_camera(entity_id) {
                let ids = [VarInt(old_cam.fake_entity_id)];
                player
                    .client
                    .send_packet_now(&CRemoveEntities::new(&ids))
                    .await;
            }

            player.camera_target_id.store(None);
            player
                .client
                .send_packet_now(&CSetCamera::new(VarInt(entity_id)))
                .await;

            sender
                .send_message(TextComponent::text(
                    "\u{00a7}aCamera reset to default\u{00a7}r",
                ))
                .await;
            Ok(1)
        })
    }
}

struct RpgClassSetExecutor {
    class_name: String,
}
impl CommandExecutor for RpgClassSetExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let Some(cls) = RpgClass::from_name(&self.class_name) else {
                sender
                    .send_message(TextComponent::text(format!(
                        "\u{00a7}cUnknown class: {}\u{00a7}r",
                        self.class_name
                    )))
                    .await;
                return Ok(0);
            };

            let entity_id = player.entity_id();
            combat::with_player_state_mut(entity_id, |state| {
                state.set_class(cls);
            });

            sender
                .send_message(TextComponent::text(format!(
                    "\u{00a7}aClass changed to {}!\u{00a7}r",
                    cls.display_name()
                )))
                .await;
            Ok(1)
        })
    }
}

struct RpgClassToggleExecutor;
impl CommandExecutor for RpgClassToggleExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let entity_id = player.entity_id();
            combat::with_player_state_mut(entity_id, |state| {
                let new_val = !state.is_enabled();
                state.set_enabled(new_val);
            });

            let is_enabled = combat::with_player_state(entity_id, |s| s.is_enabled());

            sender
                .send_message(TextComponent::text(if is_enabled {
                    "\u{00a7}aRPG system enabled!\u{00a7}r".to_string()
                } else {
                    "\u{00a7}cRPG system disabled.\u{00a7}r".to_string()
                }))
                .await;
            Ok(1)
        })
    }
}

struct RpgClassInfoExecutor;
impl CommandExecutor for RpgClassInfoExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a pumpkin::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                return Err(InvalidRequirement);
            };

            let entity_id = player.entity_id();
            let (cls_name, rpg_enabled, combo, skill_cd_info) =
                combat::with_player_state(entity_id, |state| {
                    let cls = state.get_class();
                    let name = cls.display_name().to_string();
                    let enabled = state.is_enabled();
                    let combo_count =
                        state.combo_count.load(std::sync::atomic::Ordering::Relaxed);
                    let mut cd_info = String::new();
                    for skill in &Skill::ALL {
                        let remaining = state.get_remaining_cooldown(skill.name);
                        if remaining > 0 {
                            cd_info.push_str(&format!(
                                "  {}{}: {:.1}s\n",
                                skill.damage_type.color_code(),
                                skill.name,
                                remaining as f64 / 20.0,
                            ));
                        }
                    }
                    (name, enabled, combo_count, cd_info)
                });

            let mut msg = format!(
                "\u{00a7}6=== RPG Info ===\u{00a7}r\n"
                "Class: \u{00a7}a{}\u{00a7}r\n"
                "RPG: {}\n"
                "Combo: \u{00a7}e{}x\u{00a7}r ({:.1}x damage)\n",
                cls_name,
                if rpg_enabled {
                    "\u{00a7}aON\u{00a7}r"
                } else {
                    "\u{00a7}cOFF\u{00a7}r"
                },
                combo,
                1.0 + f64::from(combo) * 0.1,
            );

            if !skill_cd_info.is_empty() {
                msg.push_str("\u{00a7}7Cooldowns:\u{00a7}r\n");
                msg.push_str(&skill_cd_info);
            }

            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

// ===== COMMAND TREE BUILDERS =====

fn build_skill_tree() -> CommandTree {
    let mut tree = CommandTree::new(["skill"], "RPG skill system");

    // /skill list
    tree = tree.then(literal("list").execute(SkillListExecutor));

    // /skill <name> — one literal per skill
    for skill in &Skill::ALL {
        let name_lower = skill.name.to_lowercase().replace(' ', "_");
        tree = tree.then(
            literal(&name_lower).execute(SkillUseExecutor {
                skill_name: skill.name.to_string(),
            }),
        );
    }

    tree
}

fn build_camera_tree() -> CommandTree {
    CommandTree::new(["camera"], "Custom camera angles")
        .then(literal("list").execute(CameraListExecutor))
        .then(literal("reset").execute(CameraResetExecutor))
        .then(
            literal("firstperson").execute(CameraSetExecutor {
                mode_name: "firstperson".to_string(),
            }),
        )
        .then(
            literal("overshoulder").execute(CameraSetExecutor {
                mode_name: "overshoulder".to_string(),
            }),
        )
        .then(
            literal("topdown").execute(CameraSetExecutor {
                mode_name: "topdown".to_string(),
            }),
        )
        .then(
            literal("cinematic").execute(CameraSetExecutor {
                mode_name: "cinematic".to_string(),
            }),
        )
        .then(
            literal("combatcam").execute(CameraSetExecutor {
                mode_name: "combatcam".to_string(),
            }),
        )
}

fn build_rpgclass_tree() -> CommandTree {
    CommandTree::new(["rpgclass"], "RPG class system")
        .then(literal("info").execute(RpgClassInfoExecutor))
        .then(literal("toggle").execute(RpgClassToggleExecutor))
        .then(
            literal("warrior").execute(RpgClassSetExecutor {
                class_name: "warrior".to_string(),
            }),
        )
        .then(
            literal("mage").execute(RpgClassSetExecutor {
                class_name: "mage".to_string(),
            }),
        )
        .then(
            literal("rogue").execute(RpgClassSetExecutor {
                class_name: "rogue".to_string(),
            }),
        )
        .then(
            literal("paladin").execute(RpgClassSetExecutor {
                class_name: "paladin".to_string(),
            }),
        )
}

// ===== REGISTRATION =====

pub async fn register_all(ctx: &pumpkin::plugin::Context) -> Result<(), String> {
    // Register commands with permission level 0 (all players)
    ctx.register_command(build_skill_tree(), "0").await;
    ctx.register_command(build_camera_tree(), "0").await;
    ctx.register_command(build_rpgclass_tree(), "0").await;
    ctx.log("Commands registered: /skill, /camera, /rpgclass");

    // Register event handlers
    ctx.register_event::<PlayerInteractEntityEvent, _>(
        Arc::new(AttackHandler),
        EventPriority::Normal,
        false,
    )
    .await;

    ctx.register_event::<ServerTickStartEvent, _>(
        Arc::new(TickHandler),
        EventPriority::Normal,
        false,
    )
    .await;

    ctx.log("Event handlers registered: Attack, Tick");

    Ok(())
}
