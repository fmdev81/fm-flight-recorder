#!/usr/bin/env python3
"""Print one line per freeze from Flight Recorder JSONL logs.

    python3 scripts/summary.py                      # today's log in ~/Library/Logs/FlightRecorder
    python3 scripts/summary.py viewer/demo.jsonl    # any log file(s)
"""
import glob, json, os, sys

files = sys.argv[1:] or sorted(glob.glob(os.path.expanduser("~/Library/Logs/FlightRecorder/flight-*.jsonl")))[-1:]
if not files:
    sys.exit("no logs found in ~/Library/Logs/FlightRecorder")
total = n = 0
for f in files:
    for line in open(f, encoding="utf-8"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get("type") != "freeze":
            continue
        b, a, net = e.get("before") or {}, e.get("after") or {}, e.get("net") or {}
        layout = a.get("layout") or b.get("layout") or "?"
        script = b.get("script") or a.get("script")
        s = f"{e['ts'][11:19]}  froze {e['duration_ms']/1000:.1f} s on {layout}"
        if b.get("layout") and a.get("layout") and b["layout"] != a["layout"]:
            s += f" (from {b['layout']})"
        if script:
            s += f", script {script}"
        rt = net.get("round_trips", 0)
        s += f", {rt} round trip{'' if rt == 1 else 's'}"
        if "wait_ms" in net:
            s += f", {net['wait_ms']/1000:.1f} s waiting on server"
        print(s)
        total += e["duration_ms"]; n += 1
print(f"-- {n} freezes, {total/1000:.1f} s frozen")
