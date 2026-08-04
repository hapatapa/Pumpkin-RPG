#!/bin/bash
# Setup script: clone Pumpkin-MC, apply patches, add plugin to workspace
set -e

PUMPKIN_REPO="https://github.com/Pumpkin-MC/Pumpkin.git"
BRANCH="master"

echo "=== Pumpkin-RPG Setup ==="

# Clone Pumpkin-MC if not already present
if [ ! -d "pumpkin" ]; then
    echo "[SETUP] Cloning Pumpkin-MC..."
    git clone --depth 1 --branch "$BRANCH" "$PUMPKIN_REPO" pumpkin
else
    echo "[SETUP] pumpkin/ already exists, skipping clone"
fi

# Apply visibility patches
echo "[SETUP] Applying patches..."
bash scripts/apply-patches.sh

# Copy plugin into the workspace
echo "[SETUP] Adding RPG plugin to workspace..."
cp -r plugin pumpkin/pumpkin-rpg-plugin

# Add plugin to workspace Cargo.toml
if ! grep -q 'pumpkin-rpg-plugin' pumpkin/Cargo.toml; then
    # Add as workspace member - insert before the closing bracket of [workspace] members
    sed -i '/^members = \[/a \  "pumpkin-rpg-plugin",' pumpkin/Cargo.toml
    echo "[SETUP] Plugin added to workspace"
else
    echo "[SETUP] Plugin already in workspace"
fi

echo ""
echo "=== Setup complete! ==="
echo "Run: cd pumpkin && cargo build --release"
echo "Binary will be at: pumpkin/target/release/pumpkin"
echo "Plugin will be at: pumpkin/target/release/libpumpkin_rpg_plugin.so"
