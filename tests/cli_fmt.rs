//! What the command's verbs print from the API's answers: the status line, the tail lines, the stats lines
//! and the page address.

use std::collections::BTreeMap;

use cache_siren::cli::{page_address, settings_line, stats_lines, status_line, tail_lines};
use cache_siren::wire::{
  Event, EventPage, Happening, Health, SessionStats, Settings, Siren, SirenPart, Stats, Usage,
};
use cache_siren::{Backend, Kind, Level};
use chrono::{Local, TimeZone};

/// Epoch milliseconds of a host-local time today, so the lines read the same in every zone.
fn at(hour: u32, minute: u32, second: u32) -> i64 {
  Local.with_ymd_and_hms(2026, 9, 11, hour, minute, second).unwrap().timestamp_millis()
}

const SETTINGS: Settings = Settings {
  armed: true,
  ttl_ms: 3_600_000,
  threshold: 100_000,
};

#[test]
fn status_names_the_version_the_switch_and_the_window() {
  let health = Health {
    ready: true,
    reason: None,
    version: "0.1.0-rev.2".into(),
  };
  let stats = Stats {
    sessions: Vec::new(),
    siren_count: 2,
    since: at(13, 52, 37),
  };
  assert_eq!(
    status_line(&health, &SETTINGS, &stats),
    "bmo-cache-siren 0.1.0-rev.2 ready · armed · threshold 100,000 · 0 sessions · 2 sirens since 13:52:37"
  );
  assert_eq!(
    settings_line(&Settings {
      armed: false,
      ..SETTINGS
    }),
    "quiet · ttl 3600000 ms · threshold 100,000"
  );
}

#[test]
fn tail_pads_cuts_and_names_each_event() {
  let usage = Usage {
    session_id: "11111111-2222".into(),
    model_name: "claude-opus-5".into(),
    model_label: "Opus 5".into(),
    at: at(13, 52, 37),
    harness_name: "claude-code".into(),
    backend: Backend::Anthropic,
    kind: Kind::Cold,
    level: Level::Red,
    note: "prefix_changed_by_a_lot".into(),
    new_token_count: 150_000,
    input_token_count: 150_000,
    cache_write_token_count: 0,
    cached_token_count: 0,
    output_token_count: 10,
    context_token_count: 150_000,
    previous_context_token_count: 0,
    gap_ms: None,
    cost_usd: 0.75,
    project_name: "a-very-long-project-name".into(),
    directory_name: "app".into(),
    subagent: false,
    parent_session_id: None,
    request_label: "⌨ check the build".into(),
    scope: None,
    actor_name: None,
    labels: BTreeMap::new(),
  };
  let siren = Siren {
    kind: Kind::Burst,
    at: at(13, 53, 1),
    session_id: None,
    project_name: "app,web".into(),
    new_token_count: 506_010,
    cached_token_count: 16_975_000,
    request_count: 4,
    note: "4 req in 60s".into(),
    parts: vec![SirenPart {
      project_name: "app".into(),
      new_token_count: 506_000,
      cached_token_count: 16_975_000,
      request_count: 3,
    }],
    audible: false,
  };
  let page = EventPage {
    items: vec![
      Event {
        id: 1,
        at: at(13, 52, 37),
        happening: Happening::Usage(Box::new(usage)),
      },
      Event {
        id: 2,
        at: at(13, 53, 1),
        happening: Happening::Siren(siren),
      },
      Event {
        id: 3,
        at: at(13, 54, 0),
        happening: Happening::Settings(SETTINGS),
      },
    ],
    more: false,
  };

  assert_eq!(
    tail_lines(&page),
    [
      "13:52:37 a-very-long- fcf2219e Opus 5    cold    + 150,000 read=        0 prefix_changed ⌨ check the build",
      "13:53:01 SIREN burst app,web +506,010 4 req in 60s (quiet)",
      "13:54:00 settings armed · ttl 3600000 ms · threshold 100,000",
    ]
  );
}

#[test]
fn stats_give_a_line_per_session() {
  let stats = Stats {
    sessions: vec![SessionStats {
      session_id: "thread-0001".into(),
      project_name: "app".into(),
      harness_name: "codex".into(),
      model_name: "gpt-5.6-sol".into(),
      request_count: 3,
      new_token_count: 1_000,
      cached_token_count: 3_000,
      output_token_count: 90,
      cached_share: Some(0.75),
      cold_count: 1,
      first_at: at(13, 0, 0),
      last_at: at(13, 5, 0),
    }],
    siren_count: 0,
    since: at(13, 0, 0),
  };
  assert_eq!(
    stats_lines(&stats),
    [
      "since 13:00:00 · 0 sirens",
      "thread-0 app          gpt-5.6-sol  req    3 new       1,000 cached       3,000 (75%) cold 1",
    ]
  );
}

#[test]
fn the_page_address_carries_the_token_in_its_fragment() {
  assert_eq!(page_address("http://127.0.0.1:9022/", "abc"), "http://127.0.0.1:9022/#access_token=abc");
}
