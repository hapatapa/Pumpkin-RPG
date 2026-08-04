#!/bin/bash
# Setup script: clone Pumpkin-MC, apply patches, add plugin to workspace
set -e

PUMPKIN_REPO="https://github.com/Pumpkin-MC/Pumpkin.git"
BRANCH="master"


echo "=== Pumpkin-RPG Setup ==="

# Clone Pumpkin-MC if not already present
if [ ! -d "pumpkin" ]; then
    echo "[SETUP] Cloning Pumpkin-MC (branch: $BRANCH)..."
    git clone --depth 1 --recurse-submodules --shallow-submodules --branch "$BRANCH" "$PUMPKIN_REPO" pumpkin
    echo "[SETUP] Clone successful"
fi

# Verify clone
if [ ! -f "pumpkin/Cargo.toml" ]; then
    echo "[ERROR] pumpkin/Cargo.toml not found after clone!"
    echo "[ERROR] Contents of pumpkin/:"
    ls -la pumpkin/ || echo "  (empty or missing)"
    exit 1
fi

echo "[SETUP] Pumpkin-MC workspace verified"

# Apply visibility patches
echo "[SETUP] Applying patches..."
bash "$(dirname "$0")/apply-patches.sh"

# Copy plugin into the workspace
echo "[SETUP] Adding RPG plugin to workspace..."
cp -r plugin pumpkin/pumpkin-rpg-plugin

# Add plugin to workspace Cargo.toml
if ! grep -q 'pumpkin-rpg-plugin' pumpkin/Cargo.toml; then
    sed -i '/^members = \[/a \  "pumpkin-rpg-plugin",' pumpkin/Cargo.toml
    echo "[SETUP] Plugin added to workspace"
    echo "[SETUP] Workspace members now:"
    grep -A15 'members' pumpkin/Cargo.toml | head -16
else
    echo "[SETUP] Plugin already in workspace"
fi

echo ""
echo "=== Setup complete! ==="
