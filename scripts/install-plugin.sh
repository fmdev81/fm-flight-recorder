#!/bin/zsh
# Copy dist/FlightRecorder.fmplugin into the per-user FileMaker Extensions
# folder. Quit FileMaker Pro first. Run it yourself; the build never does.
set -euo pipefail
cd "$(dirname "$0")/.."
SRC=dist/FlightRecorder.fmplugin
DEST="$HOME/Library/Application Support/FileMaker/Extensions"
[[ -d "$SRC" ]] || { echo "build first: scripts/build-plugin.sh" >&2; exit 1; }
if pgrep -xq "FileMaker Pro"; then
  echo "FileMaker Pro is running. Quit it, then run this again." >&2
  exit 1
fi
mkdir -p "$DEST"
rm -rf "$DEST/FlightRecorder.fmplugin"
# ditto keeps the signature; a local copy gets no quarantine attribute.
ditto "$SRC" "$DEST/FlightRecorder.fmplugin"
xattr -dr com.apple.quarantine "$DEST/FlightRecorder.fmplugin" 2>/dev/null || true
codesign --verify --strict "$DEST/FlightRecorder.fmplugin"
echo "installed to $DEST/FlightRecorder.fmplugin"
echo "logs: ~/Library/Logs/FlightRecorder/"
