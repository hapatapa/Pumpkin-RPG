#!/bin/bash
# Apply patches to Pumpkin-MC source to expose types needed by the RPG plugin.
#
# Idempotent: if the upstream already exposes the types (modern Pumpkin-MC
# does), this script skips the patch and exits 0. If upstream layout has
# drifted in a way that breaks the patch, this script fails loudly.
#
# Run this from inside the cloned `pumpkin/` directory, OR from the repo
# root with `bash scripts/apply-patches.sh` (the script auto-detects).
set -euo pipefail

# Auto-detect working directory: prefer $PWD/pumpkin if it exists, else $PWD.
if [ -d "pumpkin/pumpkin-protocol" ]; then
    cd pumpkin
elif [ ! -d "pumpkin-protocol" ]; then
    echo "[ERROR] Neither pumpkin/ nor pumpkin-protocol/ found in cwd." >&2
    echo "[ERROR] Run this from the pumpkin-rpg repo root or from inside the cloned pumpkin/ dir." >&2
    exit 1
fi

echo "[PATCH] Working directory: $(pwd)"
echo "[PATCH] Applying RPG plugin visibility patches (idempotent)..."

PROTO_CLIENT_PLAY="pumpkin-protocol/src/java/client/play/mod.rs"
PROTO_SERVER_PLAY="pumpkin-protocol/src/java/server/play/mod.rs"

# Helper: assert that a file exists.
require_file() {
    local file="$1"
    if [ ! -f "$file" ]; then
        echo "[ERROR] Required file not found: $file" >&2
        echo "[ERROR] Upstream Pumpkin-MC layout may have changed;" >&2
        echo "[ERROR] update scripts/apply-patches.sh." >&2
        exit 1
    fi
    echo "[OK] Found $file"
}

# Helper: ensure a `pub use <module>::*;` line exists after `mod <module>;`.
# Idempotent: if the pub use already exists anywhere in the file, skip.
ensure_pub_use() {
    local file="$1"
    local module="$2"

    # Check if pub use already exists (anywhere in the file).
    if grep -Eq "^pub use ${module}::\*;" "$file"; then
        echo "[OK] $module already public in $(basename "$file") (skipping)"
        return 0
    fi

    # Otherwise, find `mod <module>;` and insert pub use after it.
    if ! grep -Eq "^mod ${module};" "$file"; then
        echo "[ERROR] Neither 'mod ${module};' nor 'pub use ${module}::*;' found in $file" >&2
        echo "[ERROR] Upstream layout has changed; update scripts/apply-patches.sh." >&2
        exit 1
    fi

    sed -i "/^mod ${module};\$/a pub use ${module}::*;" "$file"

    # Verify the insertion worked.
    if ! grep -Eq "^pub use ${module}::\*;" "$file"; then
        echo "[ERROR] Failed to insert 'pub use ${module}::*;' into $file" >&2
        exit 1
    fi
    echo "[OK] Patched $module in $(basename "$file")"
}

# ---------------------------------------------------------------------------
# Pre-flight: required files must exist.
# ---------------------------------------------------------------------------
require_file "$PROTO_CLIENT_PLAY"
require_file "$PROTO_SERVER_PLAY"

# ---------------------------------------------------------------------------
# Patch 1: Make client play packet types public.
# Needed for: CSpawnEntity, CTeleportEntity, CSetCamera, CRemoveEntities
# (Used by the camera system in plugin/src/camera.rs.)
# ---------------------------------------------------------------------------
echo "[PATCH] Patch 1: client play visibility..."
ensure_pub_use "$PROTO_CLIENT_PLAY" "spawn_entity"
ensure_pub_use "$PROTO_CLIENT_PLAY" "teleport_entity"
ensure_pub_use "$PROTO_CLIENT_PLAY" "set_camera"
ensure_pub_use "$PROTO_CLIENT_PLAY" "remove_entities"

# ---------------------------------------------------------------------------
# Patch 2: Make ActionType public (needed for attack event handling).
# ---------------------------------------------------------------------------
echo "[PATCH] Patch 2: server play visibility..."
ensure_pub_use "$PROTO_SERVER_PLAY" "interact"

echo "[PATCH] All patches applied successfully (or already up-to-date)!"
