#!/bin/zsh
# Build dist/fr-server: the companion server, one file with the dashboard in it.
# Universal (arm64 + x86_64) when the x86_64-apple-darwin target is installed.
#
#   scripts/build-server.sh
#
# It does not install anything. See README.md: Run the companion server.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGETS=(aarch64-apple-darwin)
if rustup target list --installed 2>/dev/null | grep -q x86_64-apple-darwin; then
  TARGETS+=(x86_64-apple-darwin)
fi

bins=()
for t in $TARGETS; do
  cargo build --release -p fr-server --target "$t"
  bins+=("target/$t/release/fr-server")
done

mkdir -p dist
lipo -create "${bins[@]}" -output dist/fr-server
lipo -info dist/fr-server
dist/fr-server version
