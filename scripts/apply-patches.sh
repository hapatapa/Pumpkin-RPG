#!/bin/bash
# Apply patches to Pumpkin-MC source to expose types needed by the RPG plugin.
#
# Hardened version: each patch verifies it actually applied by re-checking the
# file content after the sed substitution. If the upstream file layout has
# drifted (new commit, renamed module, etc.), the script fails loudly instead
# of silently producing a broken build.
set -euo pipefail

echo "[PATCH] Applying RPG plugin visibility patches..."

PROTO_CLIENT_PLAY="pumpkin-protocol/src/java/client/play/mod.rs"
PROTO_SERVER_PLAY="pumpkin-protocol/src/java/server/play/mod.rs"

# Helper: assert that a given regex appears in a file. Exits non-zero if missing.
assert_contains() {
    local file="$1"
    local pattern="$2"
    local label="$3"
    if ! grep -Eq "$pattern" "$file"; then
        echo "[ERROR] Patch verification failed: '$label' not found in $file" >&2
        echo "[ERROR] Expected pattern: $pattern" >&2
        echo "[ERROR] Upstream may have changed; update patches/apply-patches.sh." >&2
        exit 1
    fi
}

# Helper: assert that a file exists.
require_file() {
    local file="$1"
    if [ ! -f "$file" ]; then
        echo "[ERROR] Required file not found: $file" >&2
        echo "[ERROR] Upstream layout may have changed; update patches/apply-patches.sh." >&2
        exit 1
    fi
    echo "[OK] Found $file"
}

# Pre-flight: required files must exist.
require_file "$PROTO_CLIENT_PLAY"
require_file "$PROTO_SERVER_PLAY"

# ---------------------------------------------------------------------------
# Patch 1: Make client play packet types public.
# These are needed by the plugin for camera:
#   CSpawnEntity, CTeleportEntity, CSetCamera, CRemoveEntities
# ---------------------------------------------------------------------------
echo "[PATCH] Applying client play visibility patch..."

# Each sed adds a `pub use <module>::*;` line after the matching `mod <module>;`
# declaration. We then verify the pub use actually got inserted.
sed -i '/^mod spawn_entity;$/a pub use spawn_entity::*;' "$PROTO_CLIENT_PLAY"
sed -i '/^mod teleport_entity;$/a pub use teleport_entity::*;' "$PROTO_CLIENT_PLAY"
sed -i '/^mod set_camera;$/a pub use set_camera::*;' "$PROTO_CLIENT_PLAY"
sed -i '/^mod remove_entities;$/a pub use remove_entities::*;' "$PROTO_CLIENT_PLAY"

# Verify each pub use line now exists.
assert_contains "$PROTO_CLIENT_PLAY" '^pub use spawn_entity::\*;$'    "pub use spawn_entity"
assert_contains "$PROTO_CLIENT_PLAY" '^pub use teleport_entity::\*;$' "pub use teleport_entity"
assert_contains "$PROTO_CLIENT_PLAY" '^pub use set_camera::\*;$'      "pub use set_camera"
assert_contains "$PROTO_CLIENT_PLAY" '^pub use remove_entities::\*;$' "pub use remove_entities"

echo "[OK] Client play visibility patch applied & verified."

# ---------------------------------------------------------------------------
# Patch 2: Make ActionType public (needed for attack event handling).
# ---------------------------------------------------------------------------
echo "[PATCH] Applying server play visibility patch..."

sed -i '/^mod interact;$/a pub use interact::*;' "$PROTO_SERVER_PLAY"

assert_contains "$PROTO_SERVER_PLAY" '^pub use interact::\*;$' "pub use interact"

echo "[OK] Server play visibility patch applied & verified."

echo "[PATCH] All patches applied successfully!"
