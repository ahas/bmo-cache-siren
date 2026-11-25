//! bmo-cache-siren: a live watch of prompt-cache hits and new tokens per model request, sounding a siren
//! when new tokens jump. `docs/api.md` states every route, the client, the command and the harness plugin,
//! `docs/configuration.md` every key.

pub mod attribution;
pub mod burst;
pub mod cli;
pub mod client;
pub mod clock;
pub mod clocks;
pub mod config;
pub mod data_dir;
pub mod engine;
pub mod event;
pub mod log;
pub mod mcp;
pub mod models;
pub mod openapi;
pub mod paths;
pub mod posted;
pub mod projects;
pub mod server;
pub mod session;
pub mod siren;
pub mod token;
pub mod transcript;
pub mod wire;

pub use attribution::{Cause, Flags, Kind, Request, attribute};
pub use burst::{Burst, BurstPart, Recent, burst_summary};
pub use config::{Config, TtlTable, load_config, persist_config};
pub use data_dir::DataDir;
pub use engine::{Alert, AlertHook, Engine, Filter, Refusal, Silent, load_window};
pub use event::Level;
pub use log::{Log, LogLevel};
pub use models::{Backend, MODEL_NAMES, RATES, Rates, model_name, rate_for};
pub use server::{App, router};
pub use session::Session;
pub use transcript::Origin;

/// The version the siren's health, its MCP server and its command name: the set's version the build was
/// made at, which the testbed hands the build as `BMO_VERSION`, else the manifest's placeholder (bmo-chat's
/// `docs/conventions.md`, 「Versions」).
pub const VERSION: &str = match option_env!("BMO_VERSION") {
  Some(version) => version,
  None => env!("CARGO_PKG_VERSION"),
};
