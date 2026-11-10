<p align="center"><img src="docs/images/banner.png" alt="FM-Flight-Recorder: records each FileMaker Pro freeze, by client, and splits it into server wait and client time" width="100%"></p>

# FM-Flight-Recorder

FlightRecorder is a plug-in for FileMaker Pro on macOS.
It records each freeze of the FileMaker Pro client that is 250 ms or longer.

For each freeze, the plug-in records:

- The start time and the duration of the freeze.
- The file, the layout, the script, and the account before and after the freeze.
- The number of round trips to FileMaker Server during the freeze.
- The time that the client waited for FileMaker Server.

## Status

This plug-in is a prototype.

- The plug-in loads in FileMaker Pro 26.0.3 on macOS.
- The plug-in records freezes in FileMaker Pro 26.0.3 on macOS.
- We did not test the plug-in with a file on FileMaker Server.
- The plug-in can send events to the companion server (`fr-server`) on your LAN.
- The companion server keeps the events and shows them in a dashboard.
- We did not test the plug-in and the server together at a site.

## What you need

- A Mac with macOS 11 or later. An Apple silicon Mac or an Intel Mac is satisfactory.
- FileMaker Pro 26.0.3.
- The Xcode command line tools.
- Rust 1.90 or later, with the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets.
- The Claris FileMaker Plug-in SDK 26.0.3.

## Install the tools

Do these steps one time on each Mac.

1. Open Terminal.
2. Install the Xcode command line tools:

   ```sh
   xcode-select --install
   ```

3. Install Rust. Go to <https://rustup.rs> and obey the instructions on that page.
4. Close Terminal, then open it again.
5. Add the two macOS targets:

   ```sh
   rustup target add aarch64-apple-darwin x86_64-apple-darwin
   ```

The build script always builds `aarch64-apple-darwin`.
If `x86_64-apple-darwin` is installed, the plug-in also runs on Intel Macs.

## Get the code

```sh
git clone git@github.com:solutionscay/fm-flight-recorder.git
cd fm-flight-recorder
```

## Get the Claris SDK

The Claris FileMaker Plug-in SDK is not in this repository.
The Claris license lets you use the SDK only to compile plug-ins.
Claris gives the SDK as a public download. You do not need a login.

1. Go to the repository folder in Terminal.
2. Download the SDK 26.0.3 and unzip it in the `sdk` folder:

   ```sh
   mkdir -p sdk
   curl -L -o sdk/fm_plugin_sdk_26.0.3.307.zip https://downloads.claris.com/DEVREL/sdk/fm_plugin_sdk_26.0.3.307.zip
   unzip -q sdk/fm_plugin_sdk_26.0.3.307.zip -d sdk
   ```

3. Make sure that the `sdk` folder has this layout:

   ```text
   sdk/
     PlugInSDK/
       Headers/FMWrapper/
       Libraries/Mac/FMWrapper.framework
   ```

Git ignores the `sdk` folder.
You can delete the zip file after you unzip it.

If the `sdk` folder has no SDK, the build uses the FMWrapper framework in FileMaker Pro.
The ABI check (`scripts/check-abi.sh`) needs the SDK headers.

## Build and test

Do these steps in the repository folder.

1. Run the tests of the plug-in and the server. You do not need FileMaker Pro for the tests.

   ```sh
   cargo test --workspace
   ```

2. Compare the plug-in ABI with the SDK headers:

   ```sh
   scripts/check-abi.sh
   ```

   The result is `ABI ok (arm64)` and `ABI ok (x86_64)`.

3. Build the plug-in:

   ```sh
   scripts/build-plugin.sh
   ```

The build puts the plug-in in `dist/FlightRecorder.fmplugin`.
The plug-in has an ad-hoc signature. This signature is satisfactory on your own Mac.

## Install the plug-in

1. Quit FileMaker Pro.
2. Run the install script:

   ```sh
   scripts/install-plugin.sh
   ```

The install script copies the plug-in to this folder:

`~/Library/Application Support/FileMaker/Extensions/`

If FileMaker Pro is open, the script stops and tells you to quit FileMaker Pro.
If an earlier version of the plug-in is in the folder, the script replaces it.

## Try the plug-in in FileMaker Pro

1. Start FileMaker Pro.
2. FileMaker Pro cannot verify the ad-hoc signature. Thus it shows a dialog one time:
   1. Select **Always load this plug-in**.
   2. Click **Load Plug-in**.
3. Open **FileMaker Pro > Settings** (Preferences in earlier versions).
4. Click **Plug-Ins**.
5. Make sure that **FlightRecorder** is in the list and that its checkbox is selected.
6. Close the Settings window.
7. Open a FileMaker file. A copy of a template file is satisfactory.
8. Open the Data Viewer: **Tools > Data Viewer**. You must open the file with a Full Access account.
9. In the **Watch** tab, add each calculation in the table below. Compare each result with the table.

| Calculation | Correct result |
|---|---|
| `FlightRecorder_Version` | `0.1.0` |
| `FlightRecorder_Status` | JSON that starts with `{"running":true`. The `log` value is the path of the log file. |
| `FlightRecorder_Start ( "threshold=250; port=5003; client=Test Mac" )` | The path of the log file. |
| `FlightRecorder_Start ( "threshold=250; tresh=1" )` | The path of the log file, then a second line: `ignored: tresh`. |
| `FlightRecorder_Start ( "endpoint=http://127.0.0.1:8787" )` | `error: token is required with endpoint`. The recorder does not change. |
| `FlightRecorder_SimulateFreeze ( 1500 )` | FileMaker Pro stops for 1.5 seconds. Then the log has a `freeze` record of about 1500 ms. |

After the `client=Test Mac` start, `FlightRecorder_Status` shows `"client":"Test Mac"`.

The plug-in starts to record when FileMaker Pro starts. You do not need to call `FlightRecorder_Start`.

### Look at the log

The plug-in writes one log file each day to this folder:

`~/Library/Logs/FlightRecorder/`

To see the new records, run this command in Terminal:

```sh
tail -f ~/Library/Logs/FlightRecorder/flight-*.jsonl
```

To see a summary of the newest log, run this command in the repository folder:

```sh
python3 scripts/summary.py
```

## Functions

| Function | Result |
|---|---|
| `FlightRecorder_Version` | Gives the version of the plug-in. |
| `FlightRecorder_Start ( { options } )` | Starts the recorder again. Gives the path of the log. |
| `FlightRecorder_Stop` | Stops the recorder. Gives a summary. |
| `FlightRecorder_Mark ( label )` | Writes a mark with the label and the current context to the log. |
| `FlightRecorder_Status` | Gives the status of the recorder as JSON. |
| `FlightRecorder_SimulateFreeze ( milliseconds )` | Stops FileMaker Pro for the given time. The maximum is 10000 ms. Use it only for tests. |

### Options of FlightRecorder_Start

Put the options in one text value. Put a semicolon between two options.

| Option | Default | Meaning |
|---|---|---|
| `threshold` | `250` | The shortest freeze to record, in ms. The range is 20 to 600000. |
| `port` | `5003` | The FileMaker Server port for the round-trip counter. `0` turns the counter off. Use `443` if HTTPS tunneling is on. |
| `sample_ms` | `2` | The interval to read the network counters, in ms. |
| `context_ms` | `250` | The interval to read the context (file, layout, script, account), in ms. |
| `scripts` | `0` | `1` lets the plug-in read the context while a script runs. |
| `dir` | `~/Library/Logs/FlightRecorder` | The log folder. |
| `endpoint` | none | The companion server URL. It must start with `http://`. |
| `token` | none | The Event token of the companion server. You must give it with `endpoint`. |
| `client` | the macOS computer name | The client name in the events. |

If the plug-in does not know an option, the second line of the result shows it.
If the value of `endpoint` is not correct, or `token` is missing, the result starts with `error:`. Then the recorder does not change.

## Run the companion server

The companion server (`fr-server`) receives the events from the plug-ins and shows them in a dashboard.
It is one program. The dashboard is in the program.
Run it on one Mac on the LAN. All FileMaker Pro clients must be able to connect to this Mac.

The server uses plain HTTP and has no sign-in. Each computer on the LAN can see the dashboard and the Event token.

### Build the server

Do these steps in the repository folder.

1. Build the server:

   ```sh
   scripts/build-server.sh
   ```

   The build puts the server in `dist/fr-server`.

2. Copy the server to a folder in your `PATH`:

   ```sh
   sudo mkdir -p /usr/local/bin
   sudo cp dist/fr-server /usr/local/bin/
   ```

You can copy `dist/fr-server` to a different Mac. You do not need Rust on that Mac.
If you copy the file from a zip file or from a download, remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine /usr/local/bin/fr-server
```

### Start the server

```sh
fr-server serve
```

- The server listens on port 8787 on all network interfaces.
- The first line of the output shows the dashboard address, the database file and the time zone.
- To stop the server, push Control-C.

If the macOS firewall is on, macOS asks if `fr-server` can accept incoming connections. Click **Allow**.
If you do not see this dialog, open **System Settings > Network > Firewall > Options**, and add `fr-server`.

### Open the dashboard

Open this address in a browser on the LAN. Use the LAN address of the server Mac:

`http://192.168.1.10:8787`

The pages are Overview, Layouts, Scripts, Users, Clients, Freezes and Setup.

### Connect FileMaker Pro to the server

1. In the dashboard, open **Setup**.
2. In **Startup script step**, click **Copy**.
3. In your FileMaker solution, open the script that runs at **OnFirstWindowOpen**.
4. Add a **Set Variable** step. Paste the calculation as its value.

The calculation has this form:

```
FlightRecorder_Start ( "endpoint=http://192.168.1.10:8787; token=<event token>; threshold=250; port=5003" )
```

- `endpoint` is the address of the server Mac.
- `token` is the Event token of the server.
- To give a client name, select `client=` in Setup and type the name.

After the next start of FileMaker Pro, the client shows on the **Clients** page.
`FlightRecorder_Status` in the Data Viewer shows `sent` and `last_send_error`.

### See and replace the Event token

- To see the token, open **Setup** and click **Show**.
- To see the token in Terminal, run `fr-server token`.
- To make a new token, click **New token** in Setup, or run `fr-server token --new`.

After a new token, clients with the old token get the error `401`. They stop to send until you change their startup script step.
A running server uses a token from `fr-server token --new` in 1 minute or less.

### Where the data is

The server keeps its data in this folder:

`~/Library/Application Support/FlightRecorder Server/`

- `flight-recorder.sqlite`: the database (events, clients, sessions, daily totals, settings and the Event token).
- `config.toml`: an optional config file.

To use a different folder, start the server with `--data-dir <folder>`.
To use a different database file, start the server with `--db <file>`.

The server keeps the events for 30 days and the daily totals for 730 days. You can change the two values in **Setup > Retention**.

To make a backup, stop the server and copy the folder.
You can also copy the database while the server runs:

```sh
sqlite3 ~/Library/Application\ Support/FlightRecorder\ Server/flight-recorder.sqlite ".backup /path/to/backup.sqlite"
```

### Options of the server

You can give the options on the command line or in `config.toml`.

| Option | Default | Meaning |
|---|---|---|
| `--listen` | `0.0.0.0:8787` | The address and the port. |
| `--data-dir` | `~/Library/Application Support/FlightRecorder Server` | The data folder. |
| `--db` | `<data folder>/flight-recorder.sqlite` | The database file. |
| `--tz` | the time zone of the Mac | The IANA time zone for days and hours, for example `America/Chicago`. |
| `--log-file` | none (the log goes to the terminal) | The log file. |
| `--public-url` | `http://<LAN address>:<port>` | The address in the startup script step. |

An example of `config.toml`:

```toml
listen = "0.0.0.0:8787"
tz = "America/Chicago"
event_days = 30
total_days = 730
```

Other commands:

| Command | Result |
|---|---|
| `fr-server version` | Shows the version. |
| `fr-server token` | Shows the Event token. `--new` makes a new token. |
| `fr-server import --client "Front Desk" flight-*.jsonl` | Adds the events of JSONL logs to the database. `--client` gives the client name for older logs that have no client fields. A second import of the same file adds nothing. |
| `fr-server fake` | Adds synthetic data for a demonstration. Do not use it on a server with real data. |
| `fr-server rollup` | Writes the daily totals and deletes old data now. The server also does this each hour. |

### Start the server automatically

macOS can start the server when you log in, and start it again if it stops.

1. Copy the launchd file:

   ```sh
   cp packaging/fr-server.plist ~/Library/LaunchAgents/com.solutionscay.fr-server.plist
   ```

2. In the file, replace `USER` with your macOS user name.
3. Load the file:

   ```sh
   launchctl load ~/Library/LaunchAgents/com.solutionscay.fr-server.plist
   ```

The log is in `~/Library/Logs/fr-server.log`.
To stop the server and remove the automatic start:

```sh
launchctl unload ~/Library/LaunchAgents/com.solutionscay.fr-server.plist
rm ~/Library/LaunchAgents/com.solutionscay.fr-server.plist
```

The server starts when the user logs in. To start it when the Mac starts, set the Mac to log in automatically.

### Try the server without FileMaker Pro

```sh
cargo run -p fr-server -- --data-dir /tmp/fr-demo fake
cargo run -p fr-server -- --data-dir /tmp/fr-demo serve --listen 127.0.0.1:8787
```

Then open `http://127.0.0.1:8787`. All names in the synthetic data are made up.

## Send events to the companion server

To send the events to the companion server, give the endpoint and the Event token:

`FlightRecorder_Start ( "endpoint=http://192.168.1.10:8787; token=<event token>" )`

To see if the plug-in can send, use `FlightRecorder_Status`. These values are in the JSON:

- `sent`: the number of events that the server received.
- `spooled`: the number of events in the spool. The plug-in keeps them on disk while it cannot connect to the server.
- `last_send_error`: the last error of the connection to the server.
- `write_errors` and `last_write_error`: errors of the log file.

The API is in [docs/ingest-api.md](docs/ingest-api.md).

## Log

The file name is `flight-YYYY-MM-DD.jsonl`.
The date in the file name is the local date of the Mac.
A new file starts at local midnight.
The time (`ts`) in each record is UTC.
Each line is one JSON record.
The log format is in [docs/design.md](docs/design.md).

## Remove the plug-in

1. Quit FileMaker Pro.
2. Delete the plug-in:

   ```sh
   rm -rf ~/Library/Application\ Support/FileMaker/Extensions/FlightRecorder.fmplugin
   ```

3. Optional: delete the logs and the spool of the plug-in:

   ```sh
   rm -rf ~/Library/Logs/FlightRecorder
   ```

4. Optional: delete the install id:

   ```sh
   rm -rf ~/Library/Application\ Support/FlightRecorder
   ```

5. Optional: remove the "Always load" permission:
   1. Start FileMaker Pro.
   2. Open **FileMaker Pro > Settings**, then click **Permitted**.
   3. Select the FlightRecorder entry.
   4. Push the Delete key.

## Remove the companion server

1. Stop the server. If you use launchd, remove the automatic start (see above).
2. Delete the program:

   ```sh
   sudo rm /usr/local/bin/fr-server
   ```

3. Optional: delete the data:

   ```sh
   rm -rf ~/Library/Application\ Support/FlightRecorder\ Server
   ```

## More information

- [docs/design.md](docs/design.md): the design, the log format, the server, and the test plan.
- [server/README.md](server/README.md): notes for the server code.
- [docs/sdk-notes.md](docs/sdk-notes.md): notes about the Claris plug-in SDK.
- [docs/ingest-api.md](docs/ingest-api.md): the API that the plug-in uses to send events to the companion server.
