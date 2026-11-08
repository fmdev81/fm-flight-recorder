#!/bin/zsh
# Build dist/FlightRecorder.fmplugin (universal arm64 + x86_64) and sign it.
#
#   scripts/build-plugin.sh                 # ad-hoc signature (local testing)
#   SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" scripts/build-plugin.sh
#
# It does not install anything. See scripts/install-plugin.sh.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
OUT=dist/FlightRecorder.fmplugin
TARGETS=(aarch64-apple-darwin)
if rustup target list --installed 2>/dev/null | grep -q x86_64-apple-darwin; then
  TARGETS+=(x86_64-apple-darwin)
fi

libs=()
for t in $TARGETS; do
  cargo build --release --lib --features fmx --target "$t"
  libs+=("target/$t/release/libfm_flight_recorder.dylib")
done

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS"
lipo -create "${libs[@]}" -output "$OUT/Contents/MacOS/FlightRecorder"
sed "s/@VERSION@/$VERSION/g" packaging/Info.plist > "$OUT/Contents/Info.plist"
printf 'BNDLFLRC' > "$OUT/Contents/PkgInfo"

# The bundle must expose the entry point by name.
nm -gU "$OUT/Contents/MacOS/FlightRecorder" | grep -q '_FMExternCallProc$' \
  || { echo "FMExternCallProc is not exported" >&2; exit 1; }

if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  codesign --force --timestamp --options runtime --sign "$SIGN_IDENTITY" "$OUT"
else
  codesign --force --sign - "$OUT"     # ad-hoc: enough for a local build
fi
codesign --verify --strict --verbose=2 "$OUT"
lipo -info "$OUT/Contents/MacOS/FlightRecorder"
echo "built $OUT ($VERSION)"
