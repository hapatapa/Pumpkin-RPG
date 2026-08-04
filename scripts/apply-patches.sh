#!/bin/bash
# Apply patches to Pumpkin-MC source to expose types needed by the RPG plugin
set -e

echo "[PATCH] Applying RPG plugin visibility patches..."

PROTO_CLIENT_PLAY="pumpkin-protocol/src/java/client/play/mod.rs"
PROTO_SERVER_PLAY="pumpkin-protocol/src/java/server/play/mod.rs"

# Patch 1: Make client play packet types public
# These are needed by the plugin for camera (CSpawnEntity, CTeleportEntity, CSetCamera, CRemoveEntities)
if [ -f "$PROTO_CLIENT_PLAY" ]; then
    # Add pub use for spawn_entity
    sed -i '/^mod spawn_entity;$/a pub use spawn_entity::*;' "$PROTO_CLIENT_PLAY"
    # Add pub use for teleport_entity
    sed -i '/^mod teleport_entity;$/a pub use teleport_entity::*;' "$PROTO_CLIENT_PLAY"
    # Add pub use for set_camera
    sed -i '/^mod set_camera;$/a pub use set_camera::*;' "$PROTO_CLIENT_PLAY"
    # Add pub use for remove_entities
    sed -i '/^mod remove_entities;$/a pub use remove_entities::*;' "$PROTO_CLIENT_PLAY"
    echo "[PATCH] Applied client play visibility patch"
else
    echo "[WARN] $PROTO_CLIENT_PLAY not found, skipping client play patch"
fi

# Patch 2: Make ActionType public (needed for attack event handling)
if [ -f "$PROTO_SERVER_PLAY" ]; then
    sed -i '/^mod interact;$/a pub use interact::*;' "$PROTO_SERVER_PLAY"
    echo "[PATCH] Applied server play visibility patch"
else
    echo "[WARN] $PROTO_SERVER_PLAY not found, skipping server play patch"
fi

echo "[PATCH] All patches applied successfully!"