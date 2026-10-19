//! A transcript tailed from its offset: a session's first request is its start, a cold one after the
//! lifetime is an expiry, a red row is handed to the alert hook and every row reaches the event log.

use std::fs;
use std::sync::{Arc, Mutex};

use cache_siren::{Alert, AlertHook, Config, DataDir, Engine, Kind, Log, load_recent_events};
use serde_json::{Value, json};

#[derive(Default)]
struct Recorded(Mutex<Vec<Alert>>);

impl AlertHook for Recorded {
  fn alert(&self, alert: &Alert, _muted: bool) {
    self.0.lock().unwrap().push(alert.clone());
  }
}

fn request(id: &str, ts: &str, input: u64, read: u64) -> String {
  json!({
    "type": "assistant", "timestamp": ts, "cwd": "/srv/app",
    "message": {"id": id, "model": "claude-opus-5", "content": [],
                "usage": {"input_tokens": input, "cache_creation_input_tokens": 0,
                          "cache_read_input_tokens": read, "output_tokens": 10}},
  })
  .to_string()
}

#[test]
fn a_tailed_transcript_names_its_start_and_its_expiry() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path().join("projects");
  let transcript = root.join("-srv-app").join("s1.jsonl");
  fs::create_dir_all(transcript.parent().unwrap()).unwrap();
  let typed = json!({"type": "user", "message": {"content": "check the build"}}).to_string();
  let lines = [
    typed,
    request("m1", "2026-01-01T09:00:00.000Z", 50_000, 0),
    request("m1", "2026-01-01T09:00:00.000Z", 50_000, 0),
    request("m2", "2026-01-01T11:00:00.000Z", 150_000, 0),
  ];
  fs::write(&transcript, lines.join("\n") + "\n").unwrap();

  let data = DataDir::new(dir.path().join("data"));
  let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
  let config = Config {
    projects_globs: vec![root.to_string_lossy().into_owned()],
    ..Config::default()
  };
  let alerts = Arc::new(Recorded::default());
  let engine = Engine::new(config, data.clone(), log, alerts.clone());

  assert_eq!(engine.scan_all(), 1);

  let rows = engine.rows();
  let field = |row: usize, key: &str| rows[row].get(key).cloned().unwrap_or(Value::Null);
  assert_eq!(rows.len(), 2, "one message id is one request");
  assert_eq!((field(0, "kind"), field(0, "note")), (json!("start"), json!("session_start")));
  assert_eq!((field(0, "project"), field(0, "req")), (json!("srv-app"), json!("⌨ check the build")));
  assert_eq!((field(1, "kind"), field(1, "note")), (json!("cold"), json!("ttl_expired")));
  assert_eq!((field(1, "level"), field(1, "gap_s")), (json!("red"), json!(7200)));

  let alerts = alerts.0.lock().unwrap();
  assert_eq!(alerts.iter().map(|alert| alert.kind).collect::<Vec<_>>(), [Kind::Cold]);
  assert_eq!(load_recent_events(&data.events(), 10).len(), 2);
  assert_eq!(engine.scan_all(), 1, "nothing is read twice");
  assert_eq!(engine.rows().len(), 2);
}
