//! Flight Recorder companion server: event ingest and dashboard for the LAN.
//!
//! - `api`: the JSON API (docs/ingest-api.md) and the small read calls.
//! - `store`: batches and pings into SQLite; `db`: schema and settings.
//! - `query`: facts, aggregates, series, flags; `rollup`: daily totals and
//!   retention; `tz`: the server time zone.
//! - `web`: the dashboard pages (askama templates, embedded static files).
//! - `live`: the Server-Sent Events stream that tells open pages about new data.
//! - `import`: JSONL logs; `fake`: synthetic data.

pub mod api;
pub mod app;
pub mod config;
pub mod db;
pub mod fake;
pub mod fmt;
pub mod import;
pub mod lanes;
pub mod live;
pub mod log;
pub mod query;
pub mod rollup;
pub mod store;
pub mod tz;
pub mod views;
pub mod web;
