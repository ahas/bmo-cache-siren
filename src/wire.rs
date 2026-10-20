//! The wire types (docs/api.md): every body the API takes and answers, in the common wire format. The
//! server, the client, the MCP server and the OpenAPI document all speak these, so the contract has one
//! source.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::attribution::Kind;
use crate::event::Level;
use crate::models::Backend;

/// Every failure: a non-2xx status with exactly these two keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WireError {
  /// A kebab-case code that means one thing in every service.
  pub code: String,
  /// A sentence naming what is wrong.
  pub error: String,
}

/// Readiness and version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
  pub ready: bool,
  /// Why the service is not ready; null when it is.
  pub reason: Option<String>,
  pub version: String,
}

/// One request, limit, notice, command or compact, read from a transcript or posted by a runner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
  pub session_id: String,
  pub model_name: String,
  /// The model's readable name.
  pub model_label: String,
  /// When the request went out, in epoch milliseconds; else when the line was read.
  pub at: i64,
  /// `claude-code` for a transcript, else what the poster gave or empty.
  pub harness_name: String,
  pub backend: Backend,
  pub kind: Kind,
  pub level: Level,
  /// A cause, or the text of a notice, an error, a limit or a command.
  pub note: String,
  /// Input and cache-write tokens.
  pub new_token_count: u64,
  pub input_token_count: u64,
  pub cache_write_token_count: u64,
  /// Tokens read from the cache.
  pub cached_token_count: u64,
  pub output_token_count: u64,
  pub context_token_count: u64,
  pub previous_context_token_count: u64,
  /// Milliseconds since the session's previous request.
  pub gap_ms: Option<i64>,
  /// Estimated USD for Anthropic models, 0 otherwise.
  pub cost_usd: f64,
  pub project_name: String,
  /// The last part of the working directory.
  pub directory_name: String,
  /// The transcript is a subagent's.
  pub subagent: bool,
  pub parent_session_id: Option<String>,
  /// A short label of what prompted the request.
  pub request_label: String,
  /// The poster's own grouping, never read.
  pub scope: Option<String>,
  /// Who posted it, never read.
  pub actor_name: Option<String>,
  /// The poster's own metadata, never read.
  pub labels: BTreeMap<String, String>,
}

/// The usage of one model request, as a runner posts it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewUsage {
  pub session_id: String,
  pub model_name: String,
  /// Tokens read from the cache.
  pub cached_token_count: u64,
  /// Input and cache-write tokens.
  pub new_token_count: u64,
  /// When the request went out, in epoch milliseconds.
  pub at: i64,
  /// At most `newTokenCount`; 0 where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cache_write_token_count: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub output_token_count: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub harness_name: Option<String>,
  /// The scope where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub project_name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub directory_name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub scope: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub actor_name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub labels: Option<BTreeMap<String, String>>,
}

/// One project's share of a burst window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SirenPart {
  pub project_name: String,
  pub new_token_count: u64,
  pub cached_token_count: u64,
  pub request_count: u64,
}

/// A red request or a burst that raised the siren outside its cooldown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Siren {
  /// The red request's kind, or `burst`.
  pub kind: Kind,
  pub at: i64,
  /// The request's session; null for a burst.
  pub session_id: Option<String>,
  /// The request's project, or the burst's largest owners, comma-joined.
  pub project_name: String,
  pub new_token_count: u64,
  pub cached_token_count: u64,
  pub request_count: u64,
  /// The request's cause, or `<N> req in <W>s`.
  pub note: String,
  /// A burst's projects, largest first.
  pub parts: Vec<SirenPart>,
  /// Whether sound or voice went out.
  pub audible: bool,
}

/// The siren's switch, the cache lifetime it assumes and its threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
  /// Whether a raised siren sounds and speaks.
  pub armed: bool,
  /// The cache lifetime for a harness the per-harness table does not list.
  pub ttl_ms: u64,
  /// The new tokens that make a cold or growth request red.
  pub threshold: u64,
}

/// A merge patch of the settings: a field given as null or left out stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub armed: Option<bool>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub ttl_ms: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub threshold: Option<u64>,
}

/// What an event says happened, and the member it carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data")]
pub enum Happening {
  #[serde(rename = "usage.created")]
  Usage(Box<Usage>),
  #[serde(rename = "siren.sounded")]
  Siren(Siren),
  #[serde(rename = "settings.changed")]
  Settings(Settings),
}

/// Every event kind, as `kinds` names them.
pub const EVENT_KINDS: [&str; 3] = ["usage.created", "siren.sounded", "settings.changed"];

impl Happening {
  pub fn kind(&self) -> &'static str {
    match self {
      Happening::Usage(_) => EVENT_KINDS[0],
      Happening::Siren(_) => EVENT_KINDS[1],
      Happening::Settings(_) => EVENT_KINDS[2],
    }
  }

  /// The session the event belongs to, where it belongs to one.
  pub fn session_id(&self) -> Option<&str> {
    match self {
      Happening::Usage(usage) => Some(&usage.session_id),
      Happening::Siren(siren) => siren.session_id.as_deref(),
      Happening::Settings(_) => None,
    }
  }
}

/// One event: its number, when it happened, its kind and its member.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
  /// The event number, which only grows.
  pub id: i64,
  /// When it happened, in epoch milliseconds: a Usage's own time, else when it was recorded.
  pub at: i64,
  #[serde(flatten)]
  pub happening: Happening,
}

/// A page of events, oldest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventPage {
  pub items: Vec<Event>,
  /// Whether anything stands past the last item.
  pub more: bool,
}

/// What `GET /events` is asked; the stream reads all but `limit`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventQuery {
  /// An event number: only events after it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub after: Option<i64>,
  /// A comma-separated list of event kinds.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub kinds: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub session_id: Option<String>,
  /// 1 to 1000; 100 where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub limit: Option<u32>,
}

/// One session's sums over the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
  pub session_id: String,
  pub project_name: String,
  pub harness_name: String,
  pub model_name: String,
  /// Usages with any token count.
  pub request_count: u64,
  pub new_token_count: u64,
  pub cached_token_count: u64,
  pub output_token_count: u64,
  /// Cached over cached and new tokens; null with neither.
  pub cached_share: Option<f64>,
  pub cold_count: u64,
  pub first_at: i64,
  pub last_at: i64,
}

/// Cache hits and new tokens by session since a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
  /// The latest first.
  pub sessions: Vec<SessionStats>,
  pub siren_count: u64,
  /// The time the sums start, in epoch milliseconds.
  pub since: i64,
}

/// What `GET /stats` is asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsQuery {
  /// Only events at or after this time, in epoch milliseconds.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub since: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub session_id: Option<String>,
}

/// One sounding of the siren to test it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SirenTest {
  /// Whether sound or voice went out.
  pub sounded: bool,
  pub at: i64,
}
