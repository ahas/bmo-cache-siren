//! How `<synthetic>` lines are classified; the values come from lines the CLI really left in transcripts.

use std::sync::Arc;

use cache_siren::{Config, DataDir, Engine, Event, Kind, Level, Log, Origin, Silent};
use serde_json::{Value, json};

const SID: &str = "11111111-2222-4333-8444-555555555555";

/// One `<synthetic>` assistant line as the CLI leaves it in a transcript.
fn synthetic(text: &str, is_api_error: bool, ts: &str) -> Value {
  json!({
    "type": "assistant",
    "timestamp": ts,
    "isApiErrorMessage": is_api_error,
    "cwd": "/srv/app",
    "message": {
      "id": null,
      "model": "<synthetic>",
      "role": "assistant",
      "content": [{"type": "text", "text": text}],
      "usage": {"input_tokens": 0, "output_tokens": 0,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
    },
  })
}

const AT: &str = "2026-01-01T00:00:00.000Z";

struct Fixture {
  _dir: tempfile::TempDir,
  engine: Engine,
}

fn fixture() -> Fixture {
  let dir = tempfile::tempdir().unwrap();
  let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
  let engine = Engine::new(Config::default(), DataDir::new(dir.path()), log, Arc::new(Silent));
  Fixture { _dir: dir, engine }
}

fn origin() -> Origin {
  Origin {
    sid: SID.into(),
    project: "bmo".into(),
    is_agent: false,
    parent: None,
  }
}

fn read(line: Value) -> Event {
  fixture().engine.handle(&line, &origin()).expect("the line makes a row")
}

#[test]
fn a_fold_leaves_a_notice_and_not_an_error() {
  let event = read(synthetic("No response requested.", false, AT));

  assert_eq!(event.kind, Kind::Notice);
  assert_eq!(event.level, Level::Info);
  assert_eq!(event.note, "No response requested.");
}

#[test]
fn an_api_error_is_still_an_error() {
  let text =
    "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.";
  let event = read(synthetic(text, true, AT));

  assert_eq!(event.kind, Kind::Error);
  assert_eq!(event.level, Level::Yellow);
}

#[test]
fn a_session_limit_is_still_quota() {
  let event = read(synthetic("You've hit your session limit · resets 8:10pm (UTC)", true, AT));

  assert_eq!(event.kind, Kind::Quota);
  assert_eq!(event.level, Level::Yellow);
}

#[test]
fn a_weekly_limit_is_quota_and_not_an_error() {
  let event = read(synthetic("You've hit your weekly limit · resets 6am (UTC)", true, AT));

  assert_eq!(event.kind, Kind::Quota);
  assert_eq!(event.level, Level::Yellow);
}

#[test]
fn a_monthly_spend_limit_is_quota_and_not_an_error() {
  let text = "You've hit your monthly spend limit · raise it at claude.ai/settings/usage";
  let event = read(synthetic(text, true, AT));

  assert_eq!(event.kind, Kind::Quota);
}

#[test]
fn a_failure_that_names_no_limit_is_still_an_error() {
  let event = read(synthetic("Not logged in · Please run /login", true, AT));

  assert_eq!(event.kind, Kind::Error);
}

#[test]
fn a_notice_does_not_swallow_the_error_that_follows_it() {
  let fixture = fixture();
  fixture.engine.handle(&synthetic("No response requested.", false, AT), &origin());
  let text = "API Error: Connection lost mid-response. The response above may be incomplete.";
  let event = fixture.engine.handle(&synthetic(text, true, "2026-01-01T00:00:30.000Z"), &origin());

  assert_eq!(event.map(|event| event.kind), Some(Kind::Error));
}
