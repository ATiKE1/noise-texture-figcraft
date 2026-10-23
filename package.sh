#!/bin/sh
# Pack the plugin (after ./build.sh) as dist/noise-texture-<version>.zip, with its SHA-256 next to
# it: a folder FigCraft imports (Plugins ▸ Development ▸ Import plugin from manifest…) or publishes
# to a plugin directory, the README and licences in its assets/.
set -eu
cd "$(dirname "$0")"
root=$(pwd)
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
manifest=$(sed -n 's/^ *"version": "\(.*\)",$/\1/p' plugin/figcraft-plugin.json | head -n 1)
if [ -z "$version" ] || [ "$version" != "$manifest" ]; then
    echo "The versions differ: Cargo.toml has '$version', plugin/figcraft-plugin.json '$manifest'" >&2
    exit 1
fi
if [ ! -f plugin/noise_texture.wasm ]; then
    echo "plugin/noise_texture.wasm is missing: run ./build.sh first" >&2
    exit 1
fi
name="noise-texture-$version.zip"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/noise-texture/assets" dist
cp plugin/figcraft-plugin.json plugin/noise_texture.wasm "$stage/noise-texture/"
cp README.md LICENSE-MIT LICENSE-APACHE "$stage/noise-texture/assets/"
rm -f "dist/$name" "dist/$name.sha256"
(cd "$stage" && zip -qrX "$root/dist/$name" noise-texture)
(cd dist && sha256sum "$name" > "$name.sha256")
echo "dist/$name"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "version=$version" >> "$GITHUB_OUTPUT"
fi
