#!/bin/zsh
# Check src/fmxabi.rs against the Claris SDK headers, for arm64 and x86_64.
set -euo pipefail
cd "$(dirname "$0")/.."
H=${FM_SDK_HEADERS:-sdk/PlugInSDK/Headers}
[[ -d "$H/FMWrapper" ]] || { echo "SDK headers not found at $H (see README.md: Get the Claris SDK)" >&2; exit 1; }
for a in arm64 x86_64; do
  clang++ -std=c++17 -arch $a -fsyntax-only -Wno-deprecated-declarations -I "$H" scripts/check-abi.cpp
  echo "ABI ok ($a)"
done
