//! A transcript tailed from its offset: a session's first request is its start, a cold one after the
//! lifetime is an expiry, a red Usage is handed to the alert hook and recorded as a Siren, and every
//! event reaches the event log, from which a new engine restores its window and its numbering.

use std::fs;
use std::sync::{Arc, Mutex};

use cache_siren::wire::{Happening, Usage};
use cache_siren::{Alert, AlertHook, Config, DataDir, Engine, Kind, Level, Log, Silent, load_window};
use serde_json::json;

#[derive(Default)]
struct Recorded(Mutex<Vec<Alert>>);

impl AlertHook for Recorded {
  fn alert(&self, alert: &Alert, armed: bool) -> Option<bool> {
    self.0.lock().unwrap().push(alert.clone());
    Some(armed)
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
  let engine = Engine::new(config.clone(), data.clone(), log.clone(), alerts.clone());

  assert_eq!(engine.scan_all(), 1);

  let window = engine.window();
  let usages: Vec<&Usage> = window
    .iter()
    .filter_map(|event| match &event.happening {
      Happening::Usage(usage) => Some(usage.as_ref()),
      _ => None,
    })
    .collect();
  assert_eq!(usages.len(), 2, "one message id is one request");
  assert_eq!((usages[0].kind, usages[0].note.as_str()), (Kind::Start, "session_start"));
  assert_eq!(
    (usages[0].project_name.as_str(), usages[0].request_label.as_str()),
    ("srv-app", "⌨ check the build")
  );
  assert_eq!((usages[0].session_id.as_str(), usages[0].harness_name.as_str()), ("s1", "claude-code"));
  assert_eq!((usages[1].kind, usages[1].note.as_str()), (Kind::Cold, "ttl_expired"));
  assert_eq!((usages[1].level, usages[1].gap_ms), (Level::Red, Some(7_200_000)));

  let Happening::Siren(siren) = &window[2].happening else {
    panic!("the red Usage raised a Siren")
  };
  assert_eq!((siren.kind, siren.session_id.as_deref(), siren.audible), (Kind::Cold, Some("s1"), true));
  assert_eq!(window.iter().map(|event| event.id).collect::<Vec<_>>(), [1, 2, 3]);
  assert_eq!(
    alerts.0.lock().unwrap().iter().map(|alert| alert.kind).collect::<Vec<_>>(),
    [Kind::Cold]
  );
  assert_eq!(engine.scan_all(), 1, "nothing is read twice");
  assert_eq!(engine.window().len(), 3);

  let (kept, highest) = load_window(&data.events(), 10);
  assert_eq!((kept.len(), highest), (3, 3));
  let restarted = Engine::new(config, data, log, Arc::new(Silent));
  assert_eq!(restarted.window(), window, "the window is restored from the event log");
  let next = restarted.record(Happening::Settings(restarted.settings()));
  assert_eq!(next.id, 4, "the numbering carries on past the log's highest");
}
