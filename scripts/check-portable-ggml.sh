#!/bin/bash
# CI check: fail if ggml (Whisper) was built for the build machine's CPU, since a native build crashes
# with an illegal instruction on CPUs that lack those instructions (see .cargo/config.toml).
# Usage: scripts/check-portable-ggml.sh [target-dir]   (default: target)
set -euo pipefail
shopt -s nullglob
failed=0
checked=0
for f in "${1:-target}"/*/build/whisper-rs-sys-*/out/build/CMakeCache.txt; do
  checked=1
  if grep -q '^GGML_NATIVE:BOOL=OFF$' "$f" && grep -q '^GGML_AVX512:BOOL=OFF$' "$f"; then
    echo "ok $f"
  else
    echo "NATIVE BUILD $f"
    failed=1
  fi
done
if [ "$checked" = 0 ]; then
  echo "no whisper-rs-sys CMakeCache.txt found, nothing was checked"
  exit 1
fi
exit "$failed"
