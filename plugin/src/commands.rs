//! Command registration and executors.
//!
//! Commands:
//!   /class <name>          — pick/change your class
//!   /class info            — show your current class + abilities
//!   /stats                 — show XP, level, combo, equipped class
//!   /skills                — list your class's abilities + cooldowns
//!   /skill <name>          — activate an ability (sets it as pending)
//!   /camera <mode>         — switch camera mode (firstperson/overshoulder/topdown/cinematic/combatcam)
//!   /camera reset          — reset to first person
//!   /summon boss <type>    — spawn a boss (admin permission)
//!   /rpg toggle            — enable/disable RPG systems for yourself
//!   /rpg info              — show plugin info

use std::sync::Arc;

use pumpkin::command::args::{simple::SimpleArgConsumer, Arg, ConsumedArgs};
use pumpkin::command::dispatcher::CommandError::InvalidRequirement;
use pumpkin::command::tree::builder::{argument, literal};
use pumpkin::command::tree::CommandTree;
use pumpkin::command::{CommandExecutor, CommandResult, CommandSender};
use pumpkin::plugin::Context;
use pumpkin::server::Server;
use pumpkin_util::text::TextComponent;

use crate::boss::{BossType, spawn_boss};
use crate::camera::{CameraMode, CAMERA_MANAGER};
use crate::class::{RpgClass, SkillDef};
use crate::player::{self, get_or_create, with_state, with_state_mut};

const CLASS_ARG: &str = "class_name";
const SKILL_ARG: &str = "skill_name";
const CAMERA_ARG: &str = "mode";
const BOSS_ARG: &str = "boss_type";

// === /class ===

struct ClassSetExecutor;
impl CommandExecutor for ClassSetExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let Some(Arg::Simple(name)) = args.get(CLASS_ARG) else {
                return Err(pumpkin::command::dispatcher::CommandError::InvalidConsumption(Some(CLASS_ARG.into())));
            };
            let Some(class) = RpgClass::from_name(name) else {
                sender.send_message(TextComponent::text(format!(
                    "\u{00a7}cUnknown class: {}. Try: vanguard, spellblade, trickster, evoker\u{00a7}r",
                    name
                ))).await;
                return Ok(0);
            };

            let player_uuid = player.gameprofile.id;
            let old_class = with_state(player_uuid, |s| s.get_class());
            with_state_mut(player_uuid, |s| {
                s.set_class(class);
                // Apply passive HP adjustment
                let (hp_bonus, _, _) = class.passive_stats();
                let level = s.get_level();
                let total_hp = 20.0 + hp_bonus + (level as f32 - 1.0) * 2.0;
                s.max_hp_override.store(total_hp as i32, std::sync::atomic::Ordering::Relaxed);
            });

            crate::ui::show_class_change(&player, class).await;
            // Update action bar immediately
            let state_ref = get_or_create(player_uuid);
            crate::ui::update_action_bar(&player, state_ref.value()).await;

            // Announce in chat
            let msg = format!(
                "\u{00a7}aClass changed: {}{} -> {}{}\u{00a7}r",
                old_class.color_code(), old_class.display_name(),
                class.color_code(), class.display_name()
            );
            sender.send_message(TextComponent::text(msg)).await;

            Ok(1)
        })
    }
}

struct ClassInfoExecutor;
impl CommandExecutor for ClassInfoExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let player_uuid = player.gameprofile.id;
            let class = with_state(player_uuid, |s| s.get_class());

            let mut msg = format!("\u{00a7}6=== {}{} ===\u{00a7}r\n", class.color_code(), class.display_name());
            msg.push_str(&format!("\u{00a7}7{}\u{00a7}r\n\n", class.description()));
            msg.push_str("\u{00a7}eAbilities:\u{00a7}r\n");
            for (id, name, cd) in class.all_abilities() {
                let cd_remaining = with_state(player_uuid, |s| s.cooldown_remaining_secs(id));
                let cd_str = if cd_remaining > 0.0 {
                    format!("\u{00a7}c{:.1}s\u{00a7}r", cd_remaining)
                } else {
                    "\u{00a7}aReady\u{00a7}r".to_string()
                };
                if let Some(skill) = SkillDef::by_id(id) {
                    msg.push_str(&format!("  \u{00a7}b/skill {}\u{00a7}r - {} ({}s CD) [{}] {}\n",
                        name.to_lowercase().replace(' ', "_"), name, cd, cd_str, skill.description));
                } else {
                    msg.push_str(&format!("  {} ({}s CD) [{}]\n", name, cd, cd_str));
                }
            }

            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

struct ClassListExecutor;
impl CommandExecutor for ClassListExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let mut msg = String::from("\u{00a7}6=== RPG Classes ===\u{00a7}r\n");
            for class in [RpgClass::Vanguard, RpgClass::Spellblade, RpgClass::Trickster, RpgClass::Evoker] {
                msg.push_str(&format!("  {}{:<12}\u{00a7}r - \u{00a7}7{}\u{00a7}r\n",
                    class.color_code(), class.display_name(), class.description()));
            }
            msg.push_str("\n\u{00a7}aUse /class <name> to pick.\u{00a7}r");
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

// === /stats ===

struct StatsExecutor;
impl CommandExecutor for StatsExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let player_uuid = player.gameprofile.id;

            let (class, level, xp, sp, combo, enabled) = with_state(player_uuid, |s| {
                (s.get_class(), s.get_level(), s.get_xp(), s.get_skill_points(), s.get_combo(), s.is_enabled())
            });

            let xp_needed = player::xp_to_next_level(level);
            let mut msg = format!("\u{00a7}6=== RPG Stats ===\u{00a7}r\n");
            msg.push_str(&format!("Class: {}{}\u{00a7}r\n", class.color_code(), class.display_name()));
            msg.push_str(&format!("Level: \u{00a7}a{}\u{00a7}r\n", level));
            msg.push_str(&format!("XP: \u{00a7}e{}/{}\u{00a7}r\n", xp, xp_needed));
            msg.push_str(&format!("Skill Points: \u{00a7}b{}\u{00a7}r\n", sp));
            msg.push_str(&format!("Combo: \u{00a7}e{}x\u{00a7}r ({:.1}x dmg)\n", combo, crate::damage::combo_multiplier(combo)));
            msg.push_str(&format!("RPG Enabled: {}\u{00a7}r\n",
                if enabled { "\u{00a7}aYes" } else { "\u{00a7}cNo" }));

            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

// === /skills and /skill ===

struct SkillsListExecutor;
impl CommandExecutor for SkillsListExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let player_uuid = player.gameprofile.id;
            let class = with_state(player_uuid, |s| s.get_class());

            let mut msg = format!("\u{00a7}6=== {}{} Skills ===\u{00a7}r\n", class.color_code(), class.display_name());
            for (id, name, cd) in class.all_abilities() {
                let cd_remaining = with_state(player_uuid, |s| s.cooldown_remaining_secs(id));
                let status = if cd_remaining > 0.0 {
                    format!("\u{00a7}c{:.1}s\u{00a7}r", cd_remaining)
                } else {
                    "\u{00a7}aReady\u{00a7}r".to_string()
                };
                if let Some(skill) = SkillDef::by_id(id) {
                    msg.push_str(&format!("  \u{00a7}b/skill {}\u{00a7}r - {} [{}] \u{00a7}7{}\u{00a7}r\n",
                        name.to_lowercase().replace(' ', "_"), name, status, skill.description));
                }
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
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let player_uuid = player.gameprofile.id;

            // Find the skill by name (must be in player's class)
            let class = with_state(player_uuid, |s| s.get_class());
            let lower = self.skill_name.to_lowercase().replace(' ', "_");
            let abilities = class.all_abilities();
            let found = abilities.iter().find_map(|(id, name, cd)| {
                if name.to_lowercase().replace(' ', "_") == lower {
                    Some((*id, *name, *cd))
                } else {
                    None
                }
            });

            let Some((skill_id, skill_name, _cd_secs)) = found else {
                sender.send_message(TextComponent::text(format!(
                    "\u{00a7}cYou don't have a skill named '{}'. Use /skills to see your abilities.\u{00a7}r",
                    self.skill_name
                ))).await;
                return Ok(0);
            };

            // Check cooldown
            let on_cd = with_state(player_uuid, |s| s.is_on_cooldown(skill_id));
            if on_cd {
                let remaining = with_state(player_uuid, |s| s.cooldown_remaining_secs(skill_id));
                sender.send_message(TextComponent::text(format!(
                    "\u{00a7}c{} is on cooldown! {:.1}s remaining\u{00a7}r",
                    skill_name, remaining
                ))).await;
                return Ok(0);
            }

            // Activate: set as pending + start cooldown
            let skill_def = SkillDef::by_id(skill_id);
            let cd_secs = skill_def.map_or(5.0, |s| s.cooldown_seconds);
            with_state_mut(player_uuid, |s| {
                s.pending_skill_id.store(skill_id as i32, std::sync::atomic::Ordering::Relaxed);
                s.start_cooldown(skill_id, cd_secs);
            });

            let skill_display = skill_def.map_or(skill_name, |s| s.name);
            let msg = format!(
                "\u{00a7}a{} activated! Next attack will trigger it.\u{00a7}r",
                skill_display
            );
            sender.send_message(TextComponent::text(msg)).await;

            // For self-buff skills (Bulwark, Shadowstep), apply immediately
            // rather than waiting for next attack.
            match skill_id {
                1 => { // Bulwark
                    with_state_mut(player_uuid, |s| {
                        s.bulwark_until_tick.store(
                            player::current_tick() + 100, // 5s
                            std::sync::atomic::Ordering::Relaxed
                        );
                        s.pending_skill_id.store(-1, std::sync::atomic::Ordering::Relaxed);
                    });
                    sender.send_message(TextComponent::text(
                        "\u{00a7}aBulwark active! 50% damage reduction for 5s.\u{00a7}r"
                    )).await;
                }
                4 => { // Shadowstep
                    // Teleport player 8 blocks forward in look direction
                    let pos = player.position();
                    let (yaw, pitch) = player.rotation();
                    let yaw_rad = f64::from(-yaw) * std::f64::consts::PI / 180.0;
                    let pitch_rad = f64::from(-pitch) * std::f64::consts::PI / 180.0;
                    let dir_x = -yaw_rad.sin() * pitch_rad.cos();
                    let dir_y = -pitch_rad.sin();
                    let dir_z = yaw_rad.cos() * pitch_rad.cos();
                    let new_pos = pumpkin_util::math::vector3::Vector3::new(
                        pos.x + dir_x * 8.0,
                        pos.y + dir_y * 8.0 + 1.0,
                        pos.z + dir_z * 8.0,
                    );
                    player.request_teleport(new_pos, yaw, pitch).await;
                    with_state_mut(player_uuid, |s| {
                        s.shadowstep_until_tick.store(
                            player::current_tick() + 60, // 3s crit window
                            std::sync::atomic::Ordering::Relaxed
                        );
                        s.pending_skill_id.store(-1, std::sync::atomic::Ordering::Relaxed);
                    });
                    sender.send_message(TextComponent::text(
                        "\u{00a7}aShadowstep! Next attack within 3s is a guaranteed crit.\u{00a7}r"
                    )).await;
                }
                _ => {} // Others trigger on next melee attack
            }

            Ok(1)
        })
    }
}

// === /camera ===

struct CameraSetExecutor {
    mode_name: String,
}
impl CommandExecutor for CameraSetExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let Some(mode) = CameraMode::from_name(&self.mode_name) else {
                sender.send_message(TextComponent::text(format!(
                    "\u{00a7}cUnknown camera mode: {}. Try: firstperson, overshoulder, topdown, cinematic, combatcam\u{00a7}r",
                    self.mode_name
                ))).await;
                return Ok(0);
            };

            CAMERA_MANAGER.set_mode(&player, mode).await;
            sender.send_message(TextComponent::text(format!(
                "\u{00a7}aCamera set to: {}\u{00a7}r",
                mode.display_name()
            ))).await;
            Ok(1)
        })
    }
}

struct CameraListExecutor;
impl CommandExecutor for CameraListExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let modes = [
                ("firstperson", "First Person"),
                ("overshoulder", "Over Shoulder (MC Dungeons style)"),
                ("topdown", "Top Down (isometric)"),
                ("cinematic", "Cinematic (free orbit)"),
                ("combatcam", "Combat Cam (dynamic)"),
            ];
            let mut msg = String::from("\u{00a7}6=== Camera Modes ===\u{00a7}r\n");
            for (cmd, display) in &modes {
                msg.push_str(&format!("  \u{00a7}a/camera {}\u{00a7}r - {}\n", cmd, display));
            }
            msg.push_str("  \u{00a7}a/camera reset\u{00a7}r - Reset to first person\n");
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

struct CameraResetExecutor;
impl CommandExecutor for CameraResetExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            CAMERA_MANAGER.set_mode(&player, CameraMode::FirstPerson).await;
            sender.send_message(TextComponent::text("\u{00a7}aCamera reset to First Person\u{00a7}r")).await;
            Ok(1)
        })
    }
}

// === /summon boss ===

struct SummonBossExecutor;
impl CommandExecutor for SummonBossExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, server: &'a Server, args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(Arg::Simple(type_name)) = args.get(BOSS_ARG) else {
                return Err(pumpkin::command::dispatcher::CommandError::InvalidConsumption(Some(BOSS_ARG.into())));
            };
            let Some(boss_type) = BossType::from_name(type_name) else {
                sender.send_message(TextComponent::text(format!(
                    "\u{00a7}cUnknown boss type: {}. Try: skeleton_king, corrupted_golem, wither_queen\u{00a7}r",
                    type_name
                ))).await;
                return Ok(0);
            };

            // Spawn at sender's position (or world spawn if console)
            let (pos, world) = if let Some(player) = sender.as_player() {
                (player.position(), player.world().clone())
            } else {
                let w = server.worlds.load().first().cloned();
                let Some(w) = w else {
                    sender.send_message(TextComponent::text("\u{00a7}cNo world available\u{00a7}r")).await;
                    return Ok(0);
                };
                (pumpkin_util::math::vector3::Vector3::new(0.0, 100.0, 0.0), w)
            };

            sender.send_message(TextComponent::text(format!(
                "\u{00a7}cSummoning {}...\u{00a7}r",
                boss_type.display_name()
            ))).await;

            match spawn_boss(server, boss_type, pos, &world).await {
                Some(eid) => {
                    sender.send_message(TextComponent::text(format!(
                        "\u{00a7}c{} spawned! (entity_id={})\u{00a7}r",
                        boss_type.display_name(), eid
                    ))).await;
                    Ok(1)
                }
                None => {
                    sender.send_message(TextComponent::text(
                        "\u{00a7}cFailed to spawn boss. Check server logs.\u{00a7}r"
                    )).await;
                    Ok(0)
                }
            }
        })
    }
}

// === /rpg ===

struct RpgToggleExecutor;
impl CommandExecutor for RpgToggleExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else { return Err(InvalidRequirement); };
            let player_uuid = player.gameprofile.id;
            let new_val = !with_state(player_uuid, |s| s.is_enabled());
            with_state_mut(player_uuid, |s| s.set_enabled(new_val));
            let msg = if new_val {
                "\u{00a7}aRPG system enabled!\u{00a7}r"
            } else {
                "\u{00a7}cRPG system disabled.\u{00a7}r"
            };
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

struct RpgInfoExecutor;
impl CommandExecutor for RpgInfoExecutor {
    fn execute<'a>(&'a self, sender: &'a CommandSender, _server: &'a Server, _args: &'a ConsumedArgs<'a>) -> CommandResult<'a> {
        Box::pin(async move {
            let msg = "\u{00a7}6=== Pumpkin-RPG v0.2 ===\u{00a7}r\n\
                \u{00a7}7MC Dungeons style combat, leveling, loot, bosses, and custom cameras.\u{00a7}r\n\n\
                \u{00a7}eCommands:\u{00a7}r\n\
                \u{00a7}a/class <name>\u{00a7}r - Pick a class (vanguard, spellblade, trickster, evoker)\n\
                \u{00a7}a/class info\u{00a7}r - Show your class abilities\n\
                \u{00a7}a/stats\u{00a7}r - Show your level, XP, combo\n\
                \u{00a7}a/skills\u{00a7}r - List your skills + cooldowns\n\
                \u{00a7}a/skill <name>\u{00a7}r - Activate a skill\n\
                \u{00a7}a/camera <mode>\u{00a7}r - Switch camera (try overshoulder for MC Dungeons feel)\n\
                \u{00a7}a/summon boss <type>\u{00a7}r - Spawn a boss (admin only)\n\
                \u{00a7}a/rpg toggle\u{00a7}r - Enable/disable RPG for yourself";
            sender.send_message(TextComponent::text(msg)).await;
            Ok(1)
        })
    }
}

// === Tree builders ===

fn build_class_tree() -> CommandTree {
    let mut tree = CommandTree::new(["class"], "Pick or view your RPG class");

    // /class info
    tree = tree.then(literal("info").execute(ClassInfoExecutor));

    // /class list
    tree = tree.then(literal("list").execute(ClassListExecutor));

    // /class <name>
    tree = tree.then(argument(CLASS_ARG, SimpleArgConsumer).execute(ClassSetExecutor));

    tree
}

fn build_stats_tree() -> CommandTree {
    CommandTree::new(["stats"], "Show your RPG stats")
        .execute(StatsExecutor)
}

fn build_skills_tree() -> CommandTree {
    let mut tree = CommandTree::new(["skills"], "List your class skills");
    tree = tree.then(literal("list").execute(SkillsListExecutor));
    tree.execute(SkillsListExecutor)
}

fn build_skill_tree() -> CommandTree {
    let mut tree = CommandTree::new(["skill"], "Activate a skill");

    // /skill <name> — one literal per skill
    for skill in &SkillDef::ALL {
        let name_lower = skill.name.to_lowercase().replace(' ', "_");
        tree = tree.then(literal(name_lower).execute(SkillUseExecutor {
            skill_name: skill.name.to_string(),
        }));
    }

    tree
}

fn build_camera_tree() -> CommandTree {
    CommandTree::new(["camera"], "Custom camera angles")
        .then(literal("list").execute(CameraListExecutor))
        .then(literal("reset").execute(CameraResetExecutor))
        .then(literal("firstperson").execute(CameraSetExecutor { mode_name: "firstperson".to_string() }))
        .then(literal("overshoulder").execute(CameraSetExecutor { mode_name: "overshoulder".to_string() }))
        .then(literal("topdown").execute(CameraSetExecutor { mode_name: "topdown".to_string() }))
        .then(literal("cinematic").execute(CameraSetExecutor { mode_name: "cinematic".to_string() }))
        .then(literal("combatcam").execute(CameraSetExecutor { mode_name: "combatcam".to_string() }))
}

fn build_summon_tree() -> CommandTree {
    CommandTree::new(["summonboss"], "Summon an RPG boss")
        .then(literal("boss").then(argument(BOSS_ARG, SimpleArgConsumer).execute(SummonBossExecutor)))
}

fn build_rpg_tree() -> CommandTree {
    CommandTree::new(["rpg"], "RPG system controls")
        .then(literal("toggle").execute(RpgToggleExecutor))
        .then(literal("info").execute(RpgInfoExecutor))
}

// === Registration ===

use pumpkin_util::permission::{Permission, PermissionDefault, PermissionLvl};

/// Register a permission node with the given default, then register the
/// command tree under that same node. The node is namespaced automatically
/// by the plugin name (e.g. "class.use" -> "pumpkin-rpg-plugin:class.use").
async fn register_cmd(
    ctx: &Context,
    tree: CommandTree,
    perm_node: &str,
    default: PermissionDefault,
    description: &str,
) {
    // Build the full node name. register_permission requires it to start
    // with the plugin's namespace.
    let full_node = format!("pumpkin-rpg-plugin:{perm_node}");

    // Register the permission with a default so non-OP players (or OPs,
    // depending on `default`) can actually use the command. Without this,
    // the permission check falls through to "not found" = denied.
    if let Err(e) = ctx.register_permission(Permission {
        node: full_node.clone(),
        description: description.to_string(),
        default,
        children: std::collections::HashMap::new(),
    }).await {
        ctx.log(format!("Warning: could not register permission {full_node}: {e}"));
    }

    // Register the command tree under the same permission node.
    // register_command will namespace it to "pumpkin-rpg-plugin:<perm_node>".
    ctx.register_command(tree, perm_node).await;
}

pub async fn register_all(ctx: &Context) -> Result<(), String> {
    // Player commands — everyone can use these.
    register_cmd(ctx, build_class_tree(), "class.use",
        PermissionDefault::Allow, "Pick or view your RPG class").await;
    register_cmd(ctx, build_stats_tree(), "stats.use",
        PermissionDefault::Allow, "View your RPG stats").await;
    register_cmd(ctx, build_skills_tree(), "skills.use",
        PermissionDefault::Allow, "List your class skills").await;
    register_cmd(ctx, build_skill_tree(), "skill.use",
        PermissionDefault::Allow, "Activate a skill").await;
    register_cmd(ctx, build_camera_tree(), "camera.use",
        PermissionDefault::Allow, "Switch camera mode").await;
    register_cmd(ctx, build_rpg_tree(), "rpg.use",
        PermissionDefault::Allow, "RPG system controls").await;

    // Admin command — only OPs (level 2+) can summon bosses.
    register_cmd(ctx, build_summon_tree(), "summonboss.use",
        PermissionDefault::Op(PermissionLvl::Two), "Summon an RPG boss").await;

    ctx.log("Commands registered: /class, /stats, /skills, /skill, /camera, /summonboss, /rpg");
    Ok(())
}
