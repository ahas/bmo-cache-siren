//! The HTTP contracts other services hold this daemon to.
//!
//! The collection caller posts one event per request of a runner that leaves no transcript, in the twelve
//! field names of its Event struct. The console reader asks `GET /stats` and `GET /events?limit=N` and
//! reads the fields listed here. docs/api.md states both; a change to either is a major version.

mod siren_server;

use cache_siren::{Origin, quota_from};
use serde_json::{Map, Value, json};
use siren_server::SirenServer;

/// The collection caller's Event struct, field for field.
const EVENT_FIELDS: [&str; 12] = [
  "mode",
  "model",
  "key",
  "agent",
  "project",
  "cwd",
  "session",
  "input_tokens",
  "cache_write_input_tokens",
  "cache_read_input_tokens",
  "output_tokens",
  "usage_known",
];

/// What the console reader reads from `GET /stats`, and from each row of `GET /events`.
const STATS_FIELDS: [&str; 8] =
  ["per_min", "new_60s", "sessions_30m", "uptime_s", "files", "last_event", "cfg", "quota"];
const STATS_CFG_FIELDS: [&str; 3] = ["yellow_new_tokens", "red_new_tokens", "burst_red_tokens"];
const STATS_QUOTA_FIELDS: [&str; 7] =
  ["five_h", "five_h_resets", "seven_d", "seven_d_resets", "tier", "provider", "accounts"];
const ROW_FIELDS: [&str; 17] = [
  "seq", "ts_kst", "epoch", "project", "session", "sid", "cwd", "agent", "gap_s", "mode", "m", "model",
  "kind", "level", "note", "new", "read",
];

fn event(over: Value) -> Value {
  let mut body = json!({
    "mode": "codex", "model": "gpt-5.6-sol", "key": "worker", "agent": "job-7", "project": "app",
    "cwd": "app", "session": "thread-0001", "input_tokens": 120, "cache_write_input_tokens": 0,
    "cache_read_input_tokens": 4000, "output_tokens": 30, "usage_known": true,
  });
  if let (Value::Object(body), Value::Object(over)) = (&mut body, over) {
    body.extend(over);
  }
  body
}

fn newest(feed: &Value) -> &Value {
  feed["events"].as_array().and_then(|rows| rows.last()).expect("a row")
}

#[test]
fn the_body_carries_exactly_the_event_fields() {
  let body = event(json!({}));
  let names: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
  assert_eq!(names, EVENT_FIELDS);
}

#[tokio::test]
async fn an_event_is_taken_and_shows_in_the_feed() {
  let siren = SirenServer::start(None).await;

  let (status, body) = siren.post("/events", &event(json!({}))).await;

  assert_eq!(status, 200);
  assert_eq!(body["ok"], json!(true));
  let (_, feed) = siren.get("/events?limit=5").await;
  let row = newest(&feed);
  assert_eq!(row["seq"], body["seq"]);
  assert_eq!(
    (&row["mode"], &row["agent"], &row["project"], &row["sid"]),
    (&json!("codex"), &json!("job-7"), &json!("app"), &json!("thread-0001"))
  );
  assert_eq!((&row["read"], &row["new"], &row["out"]), (&json!(4000), &json!(120), &json!(30)));
}

/// Each field but usage_known is checked by name; usage_known false marks the usage unknown.
#[tokio::test]
async fn every_field_is_read() {
  let siren = SirenServer::start(None).await;
  let mut wrong = Map::new();
  for name in &EVENT_FIELDS[..7] {
    wrong.insert((*name).into(), json!(7));
  }
  for name in &EVENT_FIELDS[7..11] {
    wrong.insert((*name).into(), json!(-1));
  }

  for (name, value) in wrong {
    let (status, body) = siren.post("/events", &event(json!({name.clone(): value}))).await;
    assert_eq!(status, 400, "{name}");
    assert!(body["error"].as_str().unwrap().contains(&name), "{name}: {body}");
  }

  let (status, _) = siren.post("/events", &event(json!({"usage_known": false}))).await;
  assert_eq!(status, 200);
  let (_, feed) = siren.get("/events?limit=1").await;
  assert_eq!(newest(&feed)["note"], json!("usage_unavailable"));
}

#[tokio::test]
async fn a_cache_write_counts_as_new_tokens() {
  let siren = SirenServer::start(None).await;

  let over = json!({"input_tokens": 10, "cache_write_input_tokens": 20, "cache_read_input_tokens": 0});
  siren.post("/events", &event(over)).await;

  let (_, feed) = siren.get("/events?limit=1").await;
  assert_eq!(newest(&feed)["new"], json!(30));
}

#[tokio::test]
async fn stats_carries_what_the_console_reads() {
  let siren = SirenServer::start(None).await;
  siren.engine().set_quota(quota_from(
    &json!({"usage": {
      "active": {"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 1},
      "quota_slots": {"1": {"five_hour_utilization": 30, "five_hour_resets_at": 1000,
                            "seven_day_utilization": 58, "seven_day_resets_at": 2000,
                            "tier": "Main account", "account": "main", "provider": "claude-code",
                            "active": true}},
    }}),
    "01:22",
  ));

  let (status, stats) = siren.get("/stats").await;

  assert_eq!(status, 200);
  for name in STATS_FIELDS {
    assert!(stats.get(name).is_some(), "{name}");
  }
  for name in STATS_CFG_FIELDS {
    assert!(stats["cfg"][name].is_u64(), "{name}");
  }
  for name in STATS_QUOTA_FIELDS {
    assert!(stats["quota"].get(name).is_some(), "{name}");
  }
  assert_eq!(stats["per_min"].as_array().map(Vec::len), Some(10));
}

#[tokio::test]
async fn every_row_carries_what_the_console_reads() {
  let siren = SirenServer::start(None).await;
  siren.post("/events", &event(json!({}))).await;
  let transcript_line = json!({
    "type": "assistant", "timestamp": "2026-01-01T00:00:00.000Z", "cwd": "/srv/app",
    "message": {"id": "msg_0001", "model": "claude-opus-5", "role": "assistant",
                "content": [{"type": "text", "text": "Done."}],
                "usage": {"input_tokens": 100, "output_tokens": 20,
                          "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
  });
  let origin = Origin {
    sid: "11111111-2222-4333-8444-555555555555".into(),
    project: "app".into(),
    is_agent: false,
    parent: None,
  };
  let row = siren.engine().handle(&transcript_line, &origin).expect("a request row");
  siren.engine().emit(row, false);

  let (status, feed) = siren.get("/events?limit=10").await;

  assert_eq!(status, 200);
  assert!(feed["seq"].is_u64());
  let rows = feed["events"].as_array().unwrap();
  assert_eq!(rows.len(), 2);
  for row in rows {
    for name in ROW_FIELDS {
      assert!(row.get(name).is_some(), "{} row lacks {name}", row["mode"]);
    }
  }
}

#[tokio::test]
async fn limit_keeps_the_newest_rows() {
  let siren = SirenServer::start(None).await;
  for n in 0..3 {
    siren.post("/events", &event(json!({"session": format!("thread-{n}")}))).await;
  }

  let (_, feed) = siren.get("/events?limit=2").await;

  let sids: Vec<&Value> = feed["events"].as_array().unwrap().iter().map(|row| &row["sid"]).collect();
  assert_eq!(sids, [&json!("thread-1"), &json!("thread-2")]);
}
