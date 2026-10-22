#!/bin/sh
# Builds the Mac MLX helper, then downloads the pinned llama.cpp Metal build, checks its sha256 (spec risk B3)
# and puts llama-server and the dylibs it links in src-tauri/engines/mac/llama/.
# The archive is kept in .cache/llama/ for the CI cache. Note: this official build needs macOS 13.3 or later.
set -eu
tag=b11819
sha=4f1c12d98435b335338bba0204dbeb91d91a07a25d50f464cd54cd73de093bcc
name="llama-$tag-bin-macos-arm64.tar.gz"
root="$(cd "$(dirname "$0")/.." && pwd)"

# First: build-engine-mac.sh recreates src-tauri/engines/mac/ from scratch.
"$root/scripts/build-engine-mac.sh"

cache="$root/.cache/llama"
mkdir -p "$cache"
if [ ! -f "$cache/$name" ]; then
  curl -fsSL --retry 3 -o "$cache/$name.part" "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$name"
  mv "$cache/$name.part" "$cache/$name"
fi
if ! echo "$sha  $cache/$name" | shasum -a 256 -c - > /dev/null; then
  rm -f "$cache/$name"
  echo "sha256 mismatch for $name, expected $sha" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
tar xzf "$cache/$name" -C "$tmp"
src="$tmp/llama-$tag"
out="$root/src-tauri/engines/mac/llama"
rm -rf "$out" && mkdir -p "$out"
cp "$src/llama-server" "$out/"
# Copy only the @rpath libraries llama-server needs (recursively), as real files under the linked name:
# the archive's version symlinks would otherwise be bundled as duplicate copies.
todo=llama-server
while [ -n "$todo" ]; do
  set -- $todo
  f=$1
  shift
  todo="$*"
  for lib in $(otool -L "$out/$f" | awk '$1 ~ /^@rpath\// { sub("@rpath/", "", $1); print $1 }'); do
    if [ ! -e "$out/$lib" ]; then
      cp -L "$src/$lib" "$out/$lib"
      todo="$todo $lib"
    fi
  done
done
cp "$src/LICENSE" "$out/LICENSE-llama.cpp"
echo "fetched $out"
