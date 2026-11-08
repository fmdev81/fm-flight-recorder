#!/usr/bin/env python3
"""Embed a JSONL log into a copy of the viewer, so it opens from file:// with no server.

    python3 scripts/make-demo-html.py viewer/demo.jsonl viewer/demo.html
    python3 scripts/make-demo-html.py ~/Library/Logs/FlightRecorder/flight-2026-11-08.jsonl /tmp/report.html
"""
import html, os, sys
src, out = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
page = open(os.path.join(here, "..", "viewer", "index.html"), encoding="utf-8").read()
log = open(src, encoding="utf-8").read().replace("</", "<\\/")
tag = f'<script id="embedded" type="application/x-ndjson" data-label="{html.escape(os.path.basename(src))}">\n{log}</script>\n'
page = page.replace("<script>\n\"use strict\";", tag + "<script>\n\"use strict\";", 1)
open(out, "w", encoding="utf-8").write(page)
print("wrote", out)
