#!/bin/sh
# Builds the Mac MLX helper and copies it, with the bundle holding MLX's Metal shaders, to src-tauri/engines/mac/.
# xcodebuild, not `swift build`: only xcodebuild compiles the Metal shaders into default.metallib.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/engine-mac"
xcodebuild -scheme shabdly-engine -configuration Release -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$root/engine-mac/build" -skipPackagePluginValidation -skipMacroValidation build
products="$root/engine-mac/build/Build/Products/Release"
out="$root/src-tauri/engines/mac"
rm -rf "$out" && mkdir -p "$out"
cp "$products/shabdly-engine" "$out/"
found=0
for b in "$products"/*.bundle; do
  if find "$b" -name '*.metallib' | grep -q .; then cp -R "$b" "$out/"; found=1; fi
done
[ "$found" = 1 ] || { echo "no bundle with a .metallib in $products" >&2; exit 1; }
echo "built $out"
