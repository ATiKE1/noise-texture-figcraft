#!/bin/sh
# Build the plugin module and put it next to its manifest in plugin/, ready to import into FigCraft
# (Plugins ▸ Development ▸ Import plugin from manifest… ▸ plugin/figcraft-plugin.json).
# Needs the WebAssembly target: rustup target add wasm32-unknown-unknown
set -eu
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown --lib
cp target/wasm32-unknown-unknown/release/noise_texture.wasm plugin/noise_texture.wasm
echo "plugin/noise_texture.wasm: $(wc -c < plugin/noise_texture.wasm) bytes"
