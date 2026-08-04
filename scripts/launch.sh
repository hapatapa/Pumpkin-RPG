#!/bin/bash
# Launch script: downloads the built plugin .so and starts the server
# Usage: ./launch.sh [server-args...]
set -e

PLUGIN_DIR="plugins"
PLUGIN_NAME="libpumpkin_rpg_plugin.so"

# Download the latest plugin from GitHub Actions artifacts
# This uses the GitHub API to find the latest successful run and download the artifact
GITHUB_REPO="hapatapa/Pumpkin-RPG"

if [ ! -f "$PLUGIN_DIR/$PLUGIN_NAME" ]; then
    echo "[LAUNCH] Plugin not found locally, downloading from GitHub Actions..."
    mkdir -p "$PLUGIN_DIR"
    
    # Get the latest successful run ID
    RUN_ID=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/actions/runs?status=success&branch=main&per_page=1" \
        | grep -o '"id": [0-9]*' | head -1 | grep -o '[0-9]*')
    
    if [ -z "$RUN_ID" ]; then
        echo "[WARN] No successful build found. Starting server without RPG plugin."
        echo "[WARN] Trigger a build at: https://github.com/$GITHUB_REPO/actions"
    else
        echo "[LAUNCH] Found build run $RUN_ID, downloading artifact..."
        
        # Get artifact download URL
        ARTIFACT_URL=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/actions/runs/$RUN_ID/artifacts" \
            | grep -o '"archive_download_url": "[^"]*"' | head -1 | cut -d'"' -f4)
        
        if [ -n "$ARTIFACT_URL" ]; then
            cd "$PLUGIN_DIR"
            curl -L -o plugin-artifact.zip "$ARTIFACT_URL"
            unzip -o plugin-artifact.zip
            # Find and move the .so file
            find . -name "$PLUGIN_NAME" -exec mv {} "$PLUGIN_NAME" \; 2>/dev/null || true
            rm -rf plugin-artifact.zip
            cd ..
            echo "[LAUNCH] Plugin downloaded successfully!"
        else
            echo "[WARN] Could not get artifact URL. Starting without plugin."
        fi
    fi
fi

if [ -f "$PLUGIN_DIR/$PLUGIN_NAME" ]; then
    echo "[LAUNCH] RPG plugin loaded from $PLUGIN_DIR/$PLUGIN_NAME"
fi

# Start the server, passing through any extra arguments
exec ./pumpkin "$@"
