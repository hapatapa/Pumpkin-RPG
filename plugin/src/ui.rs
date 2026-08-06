//! UI helpers: action bar updates, level-up notifications, combat feedback.
//!
//! The action bar shows: [Class] Lv.X | HP: cur/max | Combo: Nx
//! Updated every 10 ticks (500ms) for the player's current state.

use std::sync::Arc;

use pumpkin::entity::player::Player;
use pumpkin::plugin::api::title::TitleBuilder;
use pumpkin_util::text::TextComponent;

use crate::class::RpgClass;
use crate::player::{PlayerRpgState, current_tick};

/// Update the action bar for `player` with their current RPG state.
pub async fn update_action_bar(player: &Arc<Player>, state: &PlayerRpgState) {
    if !state.is_enabled() {
        return;
    }

    let class = state.get_class();
    let level = state.get_level();
    let xp = state.get_xp();
    let xp_needed = crate::player::xp_to_next_level(level);
    let combo = state.get_combo();

    // Format: §c[Vanguard] §fLv.5 §7(120/283 XP) §e| §fCombo: §ex3.0
    let msg = format!(
        "{}[{}] §fLv.{} §7({}/{}) §e| {}Combo: §ex{:.1}",
        class.color_code(),
        class.display_name(),
        level,
        xp,
        xp_needed,
        if combo > 0 { "§e" } else { "§7" },
        state.combo_multiplier(),
    );

    // Show pending skill if any
    let pending = state.pending_skill_id.load(std::sync::atomic::Ordering::Relaxed);
    if pending >= 0 {
        if let Some(skill) = crate::class::SkillDef::by_id(pending as usize) {
            let skill_msg = format!(" §b| {}{} ready!§r", skill.class.color_code(), skill.name);
            let full = format!("{}{}", msg, skill_msg);
            TitleBuilder::new()
                .actionbar(TextComponent::text(full))
                .send_to(player).await;
            return;
        }
    }

    TitleBuilder::new()
        .actionbar(TextComponent::text(msg))
        .send_to(player).await;
}

/// Show a level-up title + sound effect.
pub async fn show_levelup(player: &Arc<Player>, new_level: i32, class: RpgClass) {
    let title = format!("{}LEVEL UP!", class.color_code());
    let subtitle = format!("§fYou are now level §e{}§f as a {}{}§f!", new_level, class.color_code(), class.display_name());

    TitleBuilder::new()
        .title(TextComponent::text(title))
        .subtitle(TextComponent::text(subtitle))
        .times(10, 70, 20)
        .send_to(player).await;

    // Play level-up sound (using ToVEx since it's the closest vanilla fanfare)
    use pumpkin_data::sound::{Sound, SoundCategory};
    use pumpkin_util::math::vector3::Vector3;
    let pos = player.position();
    player.world().play_sound(Sound::EntityPlayerLevelup, SoundCategory::Players, &pos);
}

/// Show a class-change confirmation.
pub async fn show_class_change(player: &Arc<Player>, class: RpgClass) {
    let title = format!("{}{}", class.color_code(), class.display_name());
    let subtitle = format!("§7{}", class.description());

    TitleBuilder::new()
        .title(TextComponent::text(title))
        .subtitle(TextComponent::text(subtitle))
        .times(10, 80, 20)
        .send_to(player).await;
}

/// Show combat feedback (damage dealt, crit, etc.) as an action bar override.
/// This is a fire-and-forget — the next tick's update_action_bar will
/// overwrite it.
pub async fn show_combat_feedback(player: &Arc<Player>, message: String) {
    TitleBuilder::new()
        .actionbar(TextComponent::text(message))
        .send_to(player).await;
}
