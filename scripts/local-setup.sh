#!/bin/bash
# One-shot script: downloads everything and builds locally
# Usage: curl -sL https://raw.githubusercontent.com/hapatapa/Pumpkin-RPG/main/scripts/local-setup.sh | bash
set -e

echo "=== Pumpkin-RPG Local Build ==="
echo "This will clone Pumpkin-MC, patch it, and build with the RPG plugin."
echo "Build takes ~30-60 minutes depending on your machine."
echo ""

# Clone this repo
if [ ! -d "Pumpkin-RPG" ]; then
    git clone https://github.com/hapatapa/Pumpkin-RPG.git
cd Pumpkin-RPG
else
cd Pumpkin-RPG
    git pull
fi

# Run setup (clone pumpkin, patch, add plugin)
bash scripts/setup.sh

# Build
echo ""
echo "=== Building (this will take a while) ==="
cd pumpkin
cargo build --release --workspace 2>&1 | tee ../build.log

if [ ${PIPESTATUS[0]} -eq 0 ]; then
    echo ""
    echo "=== Build successful! ==="
    echo "Server binary: pumpkin/target/release/pumpkin"
    echo "Plugin .so:    pumpkin/target/release/libpumpkin_rpg_plugin.so"
    echo ""
    echo "To run:"
    echo "  mkdir -p plugins"
    echo "  cp pumpkin/target/release/libpumpkin_rpg_plugin.so plugins/"
    echo "  cp scripts/launch.sh pumpkin/target/release/"
    echo "  cd pumpkin/target/release && bash launch.sh"
else
    echo ""
    echo "=== Build failed! Check build.log ==="
    exit 1
fi
