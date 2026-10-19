//! bmo-cache-siren: a live watch of prompt-cache hits and new tokens per model request, sounding a siren
//! when new tokens jump. `docs/api.md` states every route and contract, `docs/configuration.md` every key.
//!
//! Derived from cache-siren, who designed
//! and built the daemon, the overlay, the live page, the request classification and the cause
//! attribution; the repository's NOTICE gives the credit in full.

pub mod attribution;
pub mod burst;
pub mod clock;
pub mod config;
pub mod log;
pub mod models;
pub mod paths;
pub mod projects;
pub mod quota;
pub mod session;

pub use attribution::{Cause, Flags, Kind, Request, attribute};
pub use burst::{Burst, BurstPart, Recent, burst_summary};
pub use config::{Config, TTL_KEYS, TtlError, TtlTable, TtlUpdate, load_config, persist_config, ttl_body};
pub use log::{Level, Log};
pub use models::{Backend, MODEL_NAMES, RATES, Rates, model_name, rate_for};
pub use quota::{QuotaReading, quota_from};
pub use session::Session;
