//! A posted usage is classified as a transcript request is, read against what the window holds of its
//! session; and the event log's broken lines cost nothing.

use std::fs;
use std::sync::Arc;

use cache_siren::wire::{Happening, NewUsage};
use cache_siren::{Backend, Config, DataDir, Engine, Kind, Level, Log, Silent, load_window};
use serde_json::json;

struct Fixture {
  _dir: tempfile::TempDir,
  engine: Engine,
}

fn fixture() -> Fixture {
  let dir = tempfile::tempdir().unwrap();
  let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
  let engine = Engine::new(Config::defaults(dir.path()), DataDir::new(dir.path()), log, Arc::new(Silent));
  Fixture { _dir: dir, engine }
}

fn usage(harness: &str, model: &str, new: u64, cached: u64, at: i64) -> NewUsage {
  NewUsage {
    session_id: "thread-1".into(),
    model_name: model.into(),
    cached_token_count: cached,
    new_token_count: new,
    at,
    harness_name: Some(harness.into()),
    ..NewUsage::default()
  }
}

#[test]
fn a_session_starts_then_reads_warm_and_goes_cold() {
  let engine = fixture().engine;
  let post = |body: NewUsage| {
    let usage = engine.posted(body).unwrap();
    engine.record_usage(usage.clone(), false);
    usage
  };

  let start = post(usage("codex", "gpt-5.6-sol", 30_000, 0, 1000));
  assert_eq!(
    (start.kind, start.level, start.note.as_str()),
    (Kind::Start, Level::Yellow, "session_start")
  );
  assert_eq!((start.backend, start.gap_ms), (Backend::Openai, None));
  let warm = post(usage("codex", "gpt-5.6-sol", 100, 30_000, 4000));
  assert_eq!(
    (warm.kind, warm.gap_ms, warm.previous_context_token_count),
    (Kind::Warm, Some(3000), 30_000)
  );
  let cold = post(usage("codex", "gpt-5.6-sol", 120_000, 0, 5000));
  assert_eq!((cold.kind, cold.level, cold.note.as_str()), (Kind::Cold, Level::Red, "unknown"));
  let quiet = post(usage("codex", "gpt-5.6-sol", 10, 0, 6000));
  assert_eq!((quiet.kind, quiet.level), (Kind::Request, Level::Info));
  let other_model = post(usage("codex", "gpt-5.6-mini", 50_000, 0, 7000));
  assert_eq!(other_model.kind, Kind::Start, "a session is seen per harness and model");

  let sirens = engine
    .window()
    .into_iter()
    .filter(|event| matches!(event.happening, Happening::Siren(_)))
    .count();
  assert_eq!(sirens, 1, "the red cold request raised the siren");
}

#[test]
fn the_split_and_the_backend_follow_what_was_posted() {
  let engine = fixture().engine;
  let mut body = usage("openrouter", "vendor/model", 30, 30, 1);
  body.cache_write_token_count = Some(20);
  body.output_token_count = Some(4);
  let posted = engine.posted(body).unwrap();
  assert_eq!(
    (posted.input_token_count, posted.cache_write_token_count, posted.output_token_count),
    (10, 20, 4)
  );
  assert_eq!((posted.backend, posted.context_token_count), (Backend::Openrouter, 60));

  let anthropic = engine.posted(usage("", "claude-opus-5", 1_000_000, 0, 1)).unwrap();
  assert_eq!(
    (anthropic.backend, anthropic.cost_usd),
    (Backend::Anthropic, 5.0),
    "priced by the model"
  );
  let first = engine.posted(usage("", "qwen-local", 10, 0, 1)).unwrap();
  engine.record_usage(first, false);
  let local = engine.posted(usage("", "qwen-local", 200_000, 0, 2)).unwrap();
  assert_eq!(
    (local.kind, local.backend, local.level),
    (Kind::Cold, Backend::Local, Level::Yellow),
    "a local backend never sounds"
  );
}

#[test]
fn a_body_breaking_a_rule_is_refused_by_its_field() {
  let engine = fixture().engine;
  let mut over = usage("codex", "m", 10, 0, 1);
  over.cache_write_token_count = Some(11);
  assert_eq!(engine.posted(over).unwrap_err(), "cacheWriteTokenCount must be at most newTokenCount");
  assert_eq!(engine.posted(usage("codex", "", 1, 0, 1)).unwrap_err(), "modelName must not be blank");
  assert!(engine.posted(usage("codex", "m", 1, 0, -1)).unwrap_err().starts_with("at "));
  let long = usage(&"h".repeat(501), "m", 1, 0, 1);
  assert_eq!(engine.posted(long).unwrap_err(), "harnessName must be at most 500 characters");
}

#[test]
fn the_newest_events_survive_broken_lines() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("events.jsonl");
  let settings = json!({"armed": true, "ttlMs": 1000, "threshold": 1});
  let event =
    |id: i64| json!({"id": id, "at": id, "kind": "settings.changed", "data": settings}).to_string();
  let lines = [
    event(1),
    "not-json".into(),
    json!({"seq": 9, "model": "older shape"}).to_string(),
    event(2),
    event(5),
  ];
  fs::write(&path, lines.join("\n") + "\n").unwrap();

  let (window, highest) = load_window(&path, 2);
  assert_eq!(window.iter().map(|event| event.id).collect::<Vec<_>>(), [2, 5]);
  assert_eq!(highest, 5);
}
