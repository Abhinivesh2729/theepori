#!/bin/sh
# Builds Thee Pori.app for this Mac with tp and tpd bundled inside it.
set -eu
cd "$(dirname "$0")/.."
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
cargo build --release -p tpd -p tp-cli
mkdir -p apps/desktop/src-tauri/binaries
cp target/release/tp "apps/desktop/src-tauri/binaries/tp-$TRIPLE"
cp target/release/tpd "apps/desktop/src-tauri/binaries/tpd-$TRIPLE"
cd apps/desktop
npm ci --no-audit --no-fund
npx tauri build
echo "Built: target/release/bundle/macos/Thee Pori.app"
