//! What kind a request is, and the name of a re-prefill's cause (docs/api.md, a Usage's `kind` and
//! `note`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Usage's or a Siren's `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
  /// New tokens under the yellow line: a normal cache hit.
  Warm,
  /// A cache read with new tokens at or past the yellow line: a big read, many tool results.
  Growth,
  /// The first request of a session.
  Start,
  /// A seen session that read nothing back and took new tokens at or past the yellow line.
  Cold,
  /// A safeguard fallback to another model, so the next request re-prefills everything.
  Refusal,
  /// A session, weekly or spend limit.
  Quota,
  /// A compact boundary or a summary.
  Compact,
  /// An API error other than a limit.
  Error,
  /// A notice the CLI wrote in place of an answer.
  Notice,
  /// A local command, such as `/model` or `/effort`.
  Cmd,
  /// A posted request whose usage is unknown.
  Request,
  /// The new tokens of the burst window past the burst line.
  Burst,
}

impl Kind {
  pub fn as_str(self) -> &'static str {
    match self {
      Kind::Warm => "warm",
      Kind::Growth => "growth",
      Kind::Start => "start",
      Kind::Cold => "cold",
      Kind::Refusal => "refusal",
      Kind::Quota => "quota",
      Kind::Compact => "compact",
      Kind::Error => "error",
      Kind::Notice => "notice",
      Kind::Cmd => "cmd",
      Kind::Request => "request",
      Kind::Burst => "burst",
    }
  }
}

/// What a session's transcript said since its previous request, read by the cause ladder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
  pub refusal: bool,
  pub effort_model: bool,
  pub compact: bool,
  pub tool_search: bool,
  pub image: bool,
}

/// The name of a re-prefill's cause, a Usage's `note` on a cold, growth or start request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
  SessionStart,
  ModelSwitch,
  EffortModel,
  Compact,
  TtlExpired,
  PrefixChanged,
  ToolSearch,
  Image,
  Unknown,
}

impl Cause {
  pub fn as_str(self) -> &'static str {
    match self {
      Cause::SessionStart => "session_start",
      Cause::ModelSwitch => "model_switch",
      Cause::EffortModel => "effort_model",
      Cause::Compact => "compact",
      Cause::TtlExpired => "ttl_expired",
      Cause::PrefixChanged => "prefix_changed",
      Cause::ToolSearch => "tool_search",
      Cause::Image => "image",
      Cause::Unknown => "unknown",
    }
  }
}

/// One request as the cause ladder sees it.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
  pub kind: Kind,
  pub flags: Flags,
  pub mode: &'a str,
  pub model: &'a str,
  /// The session's previous model; none or empty for its first request.
  pub last_model: Option<&'a str>,
  /// Seconds since the session's previous request.
  pub gap_s: Option<f64>,
  /// Cache-read tokens of this request.
  pub read: u64,
  /// The previous request's context.
  pub prev_ctx: u64,
}

/// The cause of a cold, growth or start request; none for every other kind, and for a growth request
/// nothing explains.
///
/// The sure things come first: a session's first request, a model switch, an effort switch and a
/// compact are written into the transcript as they are. The clock comes next: a gap past the lifetime
/// `ttl_s` (none where no lifetime is judged) means the cache really died. If the clock does not explain
/// it and less than `reuse_min` of the previous context was read back, the prefix moved, whatever the
/// time. A request that read all it held and only added a lot on top is just a big read. The last two
/// only explain why new tokens rose, not why the prefix was lost, so they come last.
pub fn attribute(request: &Request<'_>, ttl_s: Option<f64>, reuse_min: f64) -> Option<Cause> {
  let flags = request.flags;
  match request.kind {
    Kind::Start => return Some(Cause::SessionStart),
    Kind::Cold | Kind::Growth => {}
    _ => return None,
  }
  let switched = request.last_model.is_some_and(|last| !last.is_empty() && last != request.model);
  if flags.refusal || switched {
    return Some(Cause::ModelSwitch);
  }
  if flags.effort_model {
    return Some(Cause::EffortModel);
  }
  if flags.compact {
    return Some(Cause::Compact);
  }
  if let (Some(ttl), Some(gap)) = (ttl_s, request.gap_s)
    && gap > ttl
  {
    return Some(Cause::TtlExpired);
  }
  if request.prev_ctx > 0 && (request.read as f64) < request.prev_ctx as f64 * reuse_min {
    return Some(Cause::PrefixChanged);
  }
  if flags.tool_search {
    return Some(Cause::ToolSearch);
  }
  if flags.image {
    return Some(Cause::Image);
  }
  (request.kind == Kind::Cold).then_some(Cause::Unknown)
}
