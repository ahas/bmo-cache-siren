//! The durable ring survives broken lines, and a posted request becomes the common row.

use std::fs;
use std::sync::Arc;

use cache_siren::{Backend, Config, DataDir, Engine, Kind, Log, Silent, load_recent_events};
use serde_json::{Value, json};

#[test]
fn last_valid_rows_survive_broken_lines() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("events.jsonl");
  let lines = [
    json!({"seq": 1, "model": "old"}).to_string(),
    "not-json".into(),
    json!({"seq": 2, "model": "middle"}).to_string(),
    json!({"seq": 3, "model": "new"}).to_string(),
  ];
  fs::write(&path, lines.join("\n") + "\n").unwrap();

  let models: Vec<Value> =
    load_recent_events(&path, 2).into_iter().map(|row| row["model"].clone()).collect();
  assert_eq!(models, [json!("middle"), json!("new")]);
}

struct Fixture {
  _dir: tempfile::TempDir,
  engine: Engine,
}

fn fixture() -> Fixture {
  let dir = tempfile::tempdir().unwrap();
  let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
  let config = Config {
    ring: 20,
    ..Config::default()
  };
  let engine = Engine::new(config, DataDir::new(dir.path()), log, Arc::new(Silent));
  Fixture { _dir: dir, engine }
}

#[test]
fn codex_keeps_the_reported_split_and_agent() {
  let event = fixture()
    .engine
    .external(&json!({
      "mode": "codex",
      "model": "gpt-5.6-sol",
      "key": "bmo",
      "agent": "chat:7",
      "project": "bmo",
      "cwd": "bmo",
      "session": "thread-1",
      "input_tokens": 10,
      "cache_write_input_tokens": 20,
      "cache_read_input_tokens": 30,
      "output_tokens": 4,
      "usage_known": true,
    }))
    .unwrap();

  assert_eq!(event.mode, "codex");
  assert_eq!(event.agent, json!("chat:7"));
  assert_eq!(event.new, 30);
  assert_eq!(event.read, 30);
  assert_eq!(event.out, 4);
  assert_eq!(event.backend, Backend::Openai);
}

#[test]
fn openrouter_unknown_usage_is_not_invented() {
  let event = fixture()
    .engine
    .external(&json!({
      "mode": "openrouter",
      "model": "vendor/model",
      "key": "classifier",
      "usage_known": false,
    }))
    .unwrap();

  assert_eq!(event.kind, Kind::Request);
  assert_eq!(event.note, "usage_unavailable");
  assert_eq!(event.new, 0);
  assert_eq!(event.read, 0);
}

#[test]
fn rejects_a_provider_that_has_no_ingestion_contract() {
  let refused = fixture().engine.external(&json!({"mode": "claude-code", "model": "x", "key": "bmo"}));

  assert!(refused.is_err_and(|why| why.contains("mode")));
}
