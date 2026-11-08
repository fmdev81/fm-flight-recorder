#!/bin/zsh
# Load test inside FileMaker Pro, with one click the first time.
#
#   scripts/selftest-in-filemaker.sh [OUT_DIR]
#
# 1. installs dist/FlightRecorder.fmplugin into ~/Library/Application Support/FileMaker/Extensions
# 2. copies FileMaker's own "Tasks" template to OUT_DIR/FRSelfTest.fmp12 (never your files)
# 3. starts FileMaker Pro with FLIGHT_RECORDER_SELFTEST=1 and opens that copy
# 4. FIRST RUN ONLY: FileMaker shows "can't verify the plug-in" -> tick
#    "Always load this plug-in", click "Load Plug-in"
# 5. waits for "selftest: done" in the log, prints the session, quits FileMaker
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-/tmp/fr-selftest}
APP="/Applications/FileMaker Pro.app"
LOGDIR="$HOME/Library/Logs/FlightRecorder"
mkdir -p "$OUT"

if pgrep -xq "FileMaker Pro"; then
  echo "Quit FileMaker Pro first." >&2; exit 1
fi
[[ -d dist/FlightRecorder.fmplugin ]] || scripts/build-plugin.sh
scripts/install-plugin.sh
cp "$APP/Contents/Resources/DynamicTemplates/Tasks/en.fmp12" "$OUT/FRSelfTest.fmp12"

START=$(date -u +%Y-%m-%dT%H:%M:%S)
open --env FLIGHT_RECORDER_SELFTEST=1 -a "$APP" "$OUT/FRSelfTest.fmp12"
echo "FileMaker is starting. If it asks about the plug-in's signature:"
echo "  tick 'Always load this plug-in' and click 'Load Plug-in'."
echo "Waiting up to 3 minutes for the self-test..."

for i in {1..180}; do
  if grep -qs '"selftest: done"' "$LOGDIR"/flight-*.jsonl 2>/dev/null &&
     [[ -n "$(grep -hs 'selftest: done' "$LOGDIR"/flight-*.jsonl | awk -v s="$START" -F'"ts":"' '{split($2,a,"\""); if (a[1] >= s) print}')" ]]; then
    break
  fi
  sleep 1
done

osascript -e 'with timeout of 20 seconds' -e 'tell application "FileMaker Pro" to quit' -e 'end timeout' 2>/dev/null || true
sleep 3
pgrep -xq "FileMaker Pro" && echo "FileMaker is still running (a dialog may be open); quit it by hand."

echo
echo "== log lines since $START (UTC) =="
for f in "$LOGDIR"/flight-*.jsonl(N); do
  awk -v s="$START" -F'"ts":"' '{split($2,a,"\""); if (a[1] >= s) print}' "$f"
done | tee "$OUT/selftest-log.jsonl" | cut -c1-300
echo
python3 scripts/summary.py "$OUT/selftest-log.jsonl" || true
