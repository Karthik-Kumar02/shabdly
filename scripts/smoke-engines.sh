#!/bin/sh
# CI smoke test (spec section 5): start each helper bundled inside the built app and fail if it does not
# answer, so a broken or blocked helper never ships.
# Usage: scripts/smoke-engines.sh <engines dir of the built app>
#   Mac:     target/release/bundle/macos/Shabdly.app/Contents/Resources/engines
#   Windows: target/release/engines   (run with Git Bash)
set -eu
dir=$1
# Tiny test model, pinned (ggml-org/models moved to ggml-org/models-moved).
model_url=https://huggingface.co/ggml-org/models-moved/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf
model_sha=270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/m.gguf" "$model_url"
if command -v sha256sum > /dev/null; then sum=sha256sum; else sum="shasum -a 256"; fi
echo "$model_sha  $tmp/m.gguf" | $sum -c -

if [ "$(uname)" = Darwin ]; then
  # A missing model folder must give exit code 2: proof the helper launches and macOS does not block it.
  set +e
  SHABDLY_ENGINE_TOKEN=0000000000000000000000000000000000000000000000000000000000000000 \
    "$dir/mac/shabdly-engine" --model "$tmp/missing" --port 8095 --parent-pid $$
  code=$?
  set -e
  [ "$code" = 2 ] || { echo "shabdly-engine exited $code, expected 2" >&2; exit 1; }
  server="$dir/mac/llama/llama-server"
else
  server="$dir/win/llama/llama-server.exe"
fi

LLAMA_API_KEY=t "$server" -m "$tmp/m.gguf" --host 127.0.0.1 --port 8094 > "$tmp/llama.log" 2>&1 &
pid=$!
trap 'kill $pid 2> /dev/null || true; rm -rf "$tmp"' EXIT
for _ in $(seq 30); do
  if curl -fs http://127.0.0.1:8094/health; then echo; echo "llama-server ok"; exit 0; fi
  sleep 1
done
cat "$tmp/llama.log" >&2
echo "llama-server gave no /health 200 within 30 s" >&2
exit 1
