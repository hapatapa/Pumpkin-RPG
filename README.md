# Pumpkin-RPG

RPG combat system, skill system, and custom camera angles for [Pumpkin-MC](https://github.com/Pumpkin-MC/Pumpkin).

## Build Status

| Branch | Status |
|--------|--------|
| `main` | [![Build Pumpkin-RPG](https://github.com/hapatapa/Pumpkin-RPG/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/hapatapa/Pumpkin-RPG/actions/workflows/build.yml) |

The CI workflow builds the plugin against a pinned Pumpkin-MC commit, runs daily at 09:00 UTC to catch upstream drift early, and uploads the server binary + plugin as artifacts on every successful run.

## Features

- **RPG Classes**: Warrior, Mage, Rogue, Paladin — each with damage affinities and resistances across six damage types (Physical, Fire, Magic, Holy, Dark, Ice) in an advantage cycle.
- **Skill System**: 8 skills (Power Strike, Flame Slash, Arcane Blast, Healing Light, Shadow Strike, Frost Nova, Whirlwind, Divine Smite) with per-skill cooldowns, AoE radii, knockback, and particle effects.
- **Combo System**: Consecutive attacks within a 5-second window stack a combo multiplier (up to 2.0x at 10 hits).
- **Custom Camera Modes**: First Person, Over Shoulder, Top Down, Cinematic, Combat Cam — implemented via invisible armor Stand entities and `CSetCamera` packets.

## Commands

All commands are permission level 0 (any player can use them).

| Command | Description |
|---------|-------------|
| `/skill list` | List all available skills with damage, cooldown, and AoE info. |
| `/skill <name>` | Activate a skill (e.g. `/skill power_strike`). The next attack applies the skill effect. |
| `/camera list` | List all camera modes. |
| `/camera <mode>` | Switch to a camera mode (e.g. `/camera topdown`). |
| `/camera reset` | Reset to first-person camera. |
| `/rpgclass info` | Show your current class, RPG toggle state, combo count, and active cooldowns. |
| `/rpgclass toggle` | Enable or disable the RPG system for yourself. |
| `/rpgclass <class>` | Change your class (warrior / mage / rogue / paladin). |

## Building Locally

```bash
git clone https://github.com/hapatapa/Pumpkin-RPG.git
cd Pumpkin-RPG
bash scripts/local-setup.sh
```

See `scripts/local-setup.sh` for the exact steps the CI workflow runs.

## CI

The build is defined in `.github/workflows/build.yml`. It:

1. Pins Pumpkin-MC to a specific commit (`PUMPKIN_MC_REF` env var in the workflow) for reproducibility.
2. Clones Pumpkin-MC shallowly with submodules, with up to 3 retries on transient network failures.
3. Applies `scripts/apply-patches.sh` — this script **fails loudly** if any expected pattern is missing in upstream, instead of silently producing a broken build.
4. Copies the plugin into the Pumpkin-MC workspace and builds `pumpkin` + `pumpkin-rpg-plugin` in release mode.
5. Uploads the server binary and plugin `.so` as artifacts (30-day retention).
6. On failure, uploads `build.log` and prints the last 80 lines + first 50 error/warning lines directly in the workflow summary.

To upgrade to a newer Pumpkin-MC:

1. Update `PUMPKIN_MC_REF` in `.github/workflows/build.yml` to the new commit SHA.
2. Run `bash scripts/apply-patches.sh` locally against the new Pumpkin-MC checkout.
3. If patches fail, update the patterns in `scripts/apply-patches.sh` and re-verify.
4. Push to `main`. The workflow will rebuild against the new pinned commit.

## License

See `LICENSE` if present; otherwise this repository's code is provided as-is.
