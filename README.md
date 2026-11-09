# FileMaker Flight Recorder

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

## Build the plug-in

You need these items:

- Rust.
- The Xcode command line tools.
- FileMaker Pro 26.0.3 on the same Mac.

The Claris plug-in SDK is not in this repository.
If the SDK is not in the `sdk` folder, the build uses the FMWrapper framework in FileMaker Pro.
To get the SDK, see [docs/design.md](docs/design.md).

1. Open a terminal in the repository folder.
2. Run `cargo test`.
3. Run `scripts/build-plugin.sh`.

The build puts the plug-in in `dist/FlightRecorder.fmplugin`.

## Install the plug-in

1. Quit FileMaker Pro.
2. Run `scripts/install-plugin.sh`.
3. Start FileMaker Pro.
4. In the dialog, select **Always load this plug-in**.
5. Click **Load Plug-in**.

The install script copies the plug-in to this folder:

`~/Library/Application Support/FileMaker/Extensions/`

The plug-in starts to record when FileMaker Pro starts.

## Functions

| Function | Result |
|---|---|
| `FlightRecorder_Version` | Gives the version of the plug-in. |
| `FlightRecorder_Start ( { options } )` | Starts the recorder again. Gives the path of the log. |
| `FlightRecorder_Stop` | Stops the recorder. Gives a summary. |
| `FlightRecorder_Mark ( label )` | Writes a mark with the label and the current context to the log. |
| `FlightRecorder_Status` | Gives the status of the recorder as JSON. |
| `FlightRecorder_SimulateFreeze ( milliseconds )` | Stops FileMaker Pro for the given time. The maximum is 10000 ms. Use it only for tests. |

## Send events to the companion server

To send the events to the companion server, give the endpoint and the Event token:

`FlightRecorder_Start ( "endpoint=http://192.168.1.10:8787; token=<event token>" )`

- The endpoint must start with `http://`.
- The token is necessary when you give an endpoint.
- Add `client=<name>` to change the client name. The default is the macOS computer name.
- If an option is not correct, the result starts with `error:` and the recorder does not change.
- If the plug-in does not know an option, the second line of the result shows it, for example `ignored: tresh`.

To see if the plug-in can send, use `FlightRecorder_Status`. These values are in the JSON:

- `sent`: the number of events that the server received.
- `spooled`: the number of events in the spool. The plug-in keeps them on disk while it cannot connect to the server.
- `last_send_error`: the last error of the connection to the server.
- `write_errors` and `last_write_error`: errors of the log file.

The API is in [docs/ingest-api.md](docs/ingest-api.md).

## Log

The plug-in writes one log file each day to this folder:

`~/Library/Logs/FlightRecorder/`

The file name is `flight-YYYY-MM-DD.jsonl`.
The date in the file name is the local date of the Mac.
A new file starts at local midnight.
The time (`ts`) in each record is UTC.
Each line is one JSON record.

To see a summary of the log, run `python3 scripts/summary.py`.

## Remove the plug-in

1. Quit FileMaker Pro.
2. Delete `FlightRecorder.fmplugin` from `~/Library/Application Support/FileMaker/Extensions/`.

## More information

- [docs/design.md](docs/design.md): the design, the log format, and the test plan.
- [docs/sdk-notes.md](docs/sdk-notes.md): notes about the Claris plug-in SDK.
- [docs/ingest-api.md](docs/ingest-api.md): the API that the plug-in uses to send events to the companion server.
