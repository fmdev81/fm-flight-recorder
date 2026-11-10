# fr-server

The Flight Recorder companion server: event ingest and dashboard for the LAN.
The install steps for an admin are in the wiki: [Companion server](https://github.com/solutionscay/fm-flight-recorder/wiki/Companion-server).
The design is in [Design: Companion server](https://github.com/solutionscay/fm-flight-recorder/wiki/Design#companion-server).

## Build and run

Do these steps in the repository folder.

```sh
cargo test --workspace                     # plug-in and server tests
cargo run -p fr-server -- --data-dir /tmp/fr-dev fake
cargo run -p fr-server -- --data-dir /tmp/fr-dev serve --listen 127.0.0.1:8787
scripts/build-server.sh                    # release build: dist/fr-server
```

## Layout

| Path | What |
|---|---|
| `src/` | The server (see the module list in [Design: Companion server](https://github.com/solutionscay/fm-flight-recorder/wiki/Design#companion-server)) |
| `templates/` | askama page templates |
| `static/` | CSS and JS of the dashboard. `static/vendor/` has htmx 2.0.4 (0BSD) and uPlot 1.6.31 (MIT) with their licenses. `GET /static/licenses.txt` shows them. |
| `tests/` | `ingest.rs` (API contract), `retention.rs` (daily totals, retention, time zone), `data.rs` (import, synthetic data), `end_to_end.rs` (plug-in recorder to server to pages) |
