//! `fr-server`: the Flight Recorder companion server.

use clap::{Parser, Subcommand};
use fr_server::app::{self, App, VERSION};
use fr_server::config::{Config, Overrides};
use fr_server::tz::{now_ms, parse_ts};
use fr_server::{db, fake, import, logln, rollup};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "fr-server",
    version,
    about = "Flight Recorder companion server"
)]
struct Cli {
    /// Config file (TOML). Default: <data folder>/config.toml, if it exists.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Data folder (database and config file).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Database file. Default: <data folder>/flight-recorder.sqlite.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// IANA time zone for days and hours. Default: the zone of this machine.
    #[arg(long, global = true)]
    tz: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Receive events and serve the dashboard.
    Serve {
        /// Address and port. Default: 0.0.0.0:8787.
        #[arg(long)]
        listen: Option<String>,
        /// Write the log to this file. Default: stdout.
        #[arg(long)]
        log_file: Option<PathBuf>,
        /// The URL for the plug-in (`endpoint=`). Default: http://<LAN address>:<port>.
        #[arg(long)]
        public_url: Option<String>,
    },
    /// Print the server version.
    Version,
    /// Print the Event token. With --new, make a new one (the old one stops working).
    Token {
        #[arg(long)]
        new: bool,
    },
    /// Import JSONL logs of the plug-in.
    Import {
        /// Client name for logs that have no client fields (v1 logs).
        #[arg(long)]
        client: Option<String>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Make synthetic data: into the database, a JSONL file, or a running server.
    Fake {
        #[arg(long, default_value_t = 6)]
        clients: usize,
        #[arg(long, default_value_t = 2)]
        files: usize,
        #[arg(long, default_value_t = 7)]
        accounts: usize,
        #[arg(long, default_value_t = 10)]
        days: u32,
        #[arg(long, default_value_t = 7)]
        seed: u64,
        /// Last moment of the data (ISO-8601 UTC). Default: now.
        #[arg(long)]
        end: Option<String>,
        /// Write JSONL to this file instead.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Post to this server instead (for example http://127.0.0.1:8787).
        #[arg(long)]
        post: Option<String>,
        /// The Event token for --post.
        #[arg(long)]
        token: Option<String>,
    },
    /// Write the daily totals and apply the retention now.
    Rollup,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fr-server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let mut o = Overrides {
        config: cli.config,
        data_dir: cli.data_dir,
        db: cli.db,
        tz: cli.tz,
        ..Default::default()
    };
    match cli.cmd {
        Cmd::Version => {
            println!("fr-server {VERSION}");
            Ok(())
        }
        Cmd::Serve {
            listen,
            log_file,
            public_url,
        } => {
            o.listen = listen;
            o.log_file = log_file;
            o.public_url = public_url;
            let cfg = Config::load(&o)?;
            if let Some(p) = &cfg.log_file {
                fr_server::log::to_file(p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            let app = Arc::new(App::new(cfg)?);
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(app::serve(app, async {
                let _ = tokio::signal::ctrl_c().await;
                logln!("stopping");
            }))
        }
        Cmd::Token { new } => {
            let cfg = Config::load(&o)?;
            let c = db::open(&cfg.db)?;
            let t = if new {
                db::replace_token(&c)
            } else {
                db::ensure_token(&c)
            }
            .map_err(|e| e.to_string())?;
            println!("{t}");
            if new {
                eprintln!("New Event token. A running server uses it within 1 minute.");
            }
            Ok(())
        }
        Cmd::Import { client, files } => {
            let cfg = Config::load(&o)?;
            let app = App::new(cfg)?;
            let mut c = app.db.lock().unwrap();
            let s = app.settings(&c);
            let cutoff = app.cutoff_day(&s, now_ms());
            let mut total = import::Report::default();
            for f in &files {
                let r = import::import_file(&mut c, app.zone, cutoff, f, client.as_deref())?;
                println!(
                    "{}: {} lines, {} new, {} duplicates, {} skipped",
                    f.display(),
                    r.lines,
                    r.accepted,
                    r.duplicates,
                    r.skipped
                );
                total += r;
            }
            let rep = rollup::run(&mut c, app.zone, &s, now_ms()).map_err(|e| e.to_string())?;
            println!(
                "total: {} new, {} duplicates; {rep}",
                total.accepted, total.duplicates
            );
            Ok(())
        }
        Cmd::Fake {
            clients,
            files,
            accounts,
            days,
            seed,
            end,
            out,
            post,
            token,
        } => {
            let cfg = Config::load(&o)?;
            let zone = fr_server::tz::Zone::new(&cfg.tz)?;
            let end_ms = match end {
                Some(e) => parse_ts(&e)
                    .ok_or("--end must be ISO-8601, for example 2026-11-06T22:58:00Z")?,
                None => now_ms(),
            };
            let opts = fake::Opts {
                clients,
                files,
                accounts,
                days,
                seed,
                end_ms,
            };
            let sessions = fake::generate(&opts, zone);
            if let Some(p) = out {
                std::fs::write(&p, fake::to_jsonl(&sessions)).map_err(|e| e.to_string())?;
                println!("wrote {}", p.display());
                return Ok(());
            }
            if let Some(url) = post {
                let token = token.ok_or("--post needs --token")?;
                let n = fake::post(&sessions, &url, &token, end_ms)?;
                println!("posted {n} new events to {url}");
                return Ok(());
            }
            let app = App::new(cfg)?;
            let mut c = app.db.lock().unwrap();
            let s = app.settings(&c);
            let now = now_ms();
            let n = fake::store(&mut c, zone, app.cutoff_day(&s, now), &sessions, end_ms)?;
            let rep = rollup::run(&mut c, zone, &s, now).map_err(|e| e.to_string())?;
            println!("stored {n} new events in {}; {rep}", app.cfg.db.display());
            Ok(())
        }
        Cmd::Rollup => {
            let cfg = Config::load(&o)?;
            let app = App::new(cfg)?;
            let mut c = app.db.lock().unwrap();
            let s = app.settings(&c);
            let now = now_ms();
            rollup::close_offline_sessions(&c, now, app.connected_ms())
                .map_err(|e| e.to_string())?;
            rollup::close_open_freezes(&c, now).map_err(|e| e.to_string())?;
            let rep = rollup::run(&mut c, app.zone, &s, now).map_err(|e| e.to_string())?;
            println!("{rep}");
            Ok(())
        }
    }
}
