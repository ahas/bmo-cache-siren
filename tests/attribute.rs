//! The cause ladder and the TTL choice; the values come from real requests.

use cache_siren::{Cause, Config, Flags, Kind, Request, attribute};
use serde_json::json;

const TTL: f64 = 3600.0;

fn request<'a>(
  kind: Kind,
  flags: Flags,
  model: &'a str,
  last: Option<&'a str>,
  gap: Option<f64>,
) -> Request<'a> {
  Request {
    kind,
    flags,
    mode: "claude-code",
    model,
    last_model: last,
    gap_s: gap,
    read: 0,
    prev_ctx: 0,
  }
}

/// The cause under the default configuration, with `read` of `prev_ctx` read back.
fn named(
  kind: Kind,
  flags: Flags,
  model: &str,
  last: Option<&str>,
  gap: Option<f64>,
  read: u64,
  prev: u64,
) -> Option<Cause> {
  Config::default().attribute(&Request {
    read,
    prev_ctx: prev,
    ..request(kind, flags, model, last, gap)
  })
}

const NONE: Flags = Flags {
  refusal: false,
  effort_model: false,
  compact: false,
  tool_search: false,
  image: false,
};

#[test]
fn ttl_is_chosen_by_mode_and_longest_model_prefix() {
  let config = Config {
    ttl_s: 99.into(),
    ttl_by_mode_model_s: serde_json::from_value(json!({
      "claude-code": {"claude-sonnet": 600, "claude-sonnet-5": 300, "*": 3600},
      "codex": {"gpt-5.6": 1800},
      "openrouter": {},
    }))
    .unwrap(),
    ..Config::default()
  };

  assert_eq!(config.ttl_for("claude-code", "claude-sonnet-5-20260901"), Some(300.0));
  assert_eq!(config.ttl_for("claude-code", "claude-opus-5"), Some(3600.0));
  assert_eq!(config.ttl_for("codex", "gpt-5.6-sol"), Some(1800.0));
  assert_eq!(config.ttl_for("openrouter", "anthropic/claude-sonnet-5"), None);
  assert_eq!(config.ttl_for("unlisted", "some-model"), Some(99.0));
}

#[test]
fn sonnet_follows_the_same_default_hour_as_opus() {
  let sonnet = Some("claude-sonnet-5");
  assert_eq!(
    named(Kind::Cold, NONE, "claude-sonnet-5", sonnet, Some(301.0), 0, 0),
    Some(Cause::Unknown)
  );
  assert_eq!(
    named(Kind::Cold, NONE, "claude-sonnet-5", sonnet, Some(TTL + 1.0), 0, 0),
    Some(Cause::TtlExpired)
  );
}

/// A gap of 3382s is inside an hour, and it held 34177 but read only 2315: the cache was alive, so this
/// is not an expiry but a changed prefix.
#[test]
fn owner_report_is_prefix_changed() {
  let opus = Some("claude-opus-5");
  assert_eq!(
    named(Kind::Growth, NONE, "claude-opus-5", opus, Some(3382.0), 2315, 34177),
    Some(Cause::PrefixChanged)
  );
}

#[test]
fn gap_past_ttl_is_ttl_expired() {
  let opus = Some("claude-opus-5");
  assert_eq!(
    named(Kind::Cold, NONE, "claude-opus-5", opus, Some(TTL + 1.0), 0, 40410),
    Some(Cause::TtlExpired)
  );
}

/// Expiry needs a gap past the TTL: a gap exactly at it still has the cache.
#[test]
fn gap_exactly_at_ttl_is_not_ttl_expired() {
  let opus = Some("claude-opus-5");
  assert_ne!(
    named(Kind::Growth, NONE, "claude-opus-5", opus, Some(TTL), 62948, 62950),
    Some(Cause::TtlExpired)
  );
}

/// A request that read all it held and added a lot on top is just a big read.
#[test]
fn reused_prefix_with_big_read_keeps_its_name() {
  let opus = Some("claude-opus-5");
  assert_eq!(named(Kind::Growth, NONE, "claude-opus-5", opus, Some(TTL - 1.0), 62948, 62950), None);
  let tool_search = Flags {
    tool_search: true,
    ..NONE
  };
  assert_eq!(
    named(Kind::Cold, tool_search, "claude-opus-5", opus, Some(600.0), 308616, 308618),
    Some(Cause::ToolSearch)
  );
}

#[test]
fn first_request_is_session_start() {
  assert_eq!(named(Kind::Start, NONE, "claude-opus-5", None, None, 0, 0), Some(Cause::SessionStart));
}

#[test]
fn model_switch_wins() {
  let opus = Some("claude-opus-5");
  assert_eq!(
    named(Kind::Cold, NONE, "claude-sonnet-5", opus, Some(120.0), 0, 34177),
    Some(Cause::ModelSwitch)
  );
  let refusal = Flags {
    refusal: true,
    ..NONE
  };
  assert_eq!(
    named(Kind::Growth, refusal, "claude-opus-5", opus, Some(120.0), 2315, 34177),
    Some(Cause::ModelSwitch)
  );
}

#[test]
fn compact_wins() {
  let opus = Some("claude-opus-5");
  let compact = Flags {
    compact: true,
    ..NONE
  };
  assert_eq!(
    named(Kind::Cold, compact, "claude-opus-5", opus, Some(120.0), 0, 34177),
    Some(Cause::Compact)
  );
  let effort = Flags {
    effort_model: true,
    ..NONE
  };
  assert_eq!(
    named(Kind::Cold, effort, "claude-opus-5", opus, Some(120.0), 0, 34177),
    Some(Cause::EffortModel)
  );
}

/// Inside the cache lifetime yet nothing was read: the whole prefix moved.
#[test]
fn cold_inside_ttl_is_prefix_changed() {
  let opus = Some("claude-opus-5");
  assert_eq!(
    named(Kind::Cold, NONE, "claude-opus-5", opus, Some(300.0), 0, 693617),
    Some(Cause::PrefixChanged)
  );
}

/// With no previous context, the first usage after an error or a limit, there is no ground to judge the
/// prefix.
#[test]
fn unknown_survives_for_a_first_context() {
  let opus = Some("claude-opus-5");
  assert_eq!(named(Kind::Cold, NONE, "claude-opus-5", opus, Some(300.0), 0, 0), Some(Cause::Unknown));
}

#[test]
fn ttl_is_configurable() {
  let cold = request(Kind::Cold, NONE, "claude-opus-5", Some("claude-opus-5"), Some(3400.0));
  let reuse = Config::default().prefix_reuse_min;
  assert_eq!(attribute(&cold, Some(3300.0), reuse), Some(Cause::TtlExpired));
  assert_eq!(attribute(&cold, Some(7200.0), reuse), Some(Cause::Unknown));
}

#[test]
fn other_kinds_get_no_name() {
  let opus = Some("claude-opus-5");
  assert_eq!(named(Kind::Warm, NONE, "claude-opus-5", opus, Some(7200.0), 100, 200000), None);
}
