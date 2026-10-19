//! `GET` and `POST /ttl`: the cache lifetime table is read, applied live, kept in the config file, and a
//! bad body changes nothing. The config file is still read at start, so a writer that edits the file and
//! restarts the service keeps working.

mod siren_server;

use std::fs;

use cache_siren::{Cause, Flags, Kind, Log, Request, load_config};
use serde_json::{Value, json};
use siren_server::SirenServer;

fn table() -> Value {
  json!({
    "claude-code": {"claude-sonnet-5": 300, "*": 3600},
    "codex": {"gpt-5.6": 1800},
    "openrouter": {},
  })
}

/// No `.tmp` file is left in `dir`.
fn no_temporary_in(dir: &std::path::Path) -> bool {
  fs::read_dir(dir)
    .unwrap()
    .all(|entry| !entry.unwrap().file_name().to_string_lossy().contains(".tmp"))
}

/// The config file read again, as a restart reads it.
fn read_at_start(siren: &SirenServer) -> cache_siren::Config {
  let log = Log::new(siren.dir.path().join("cache-siren.log"), true);
  load_config(&siren.config_path, SirenServer::defaults(&siren.dir), &log)
}

#[tokio::test]
async fn get_answers_the_table_read_at_start() {
  let siren = SirenServer::start(Some(json!({"ttl_s": 1200, "ttl_by_mode_model_s": table()}))).await;

  let (status, body) = siren.get("/ttl").await;

  assert_eq!(status, 200);
  assert_eq!(body, json!({"ttl_s": 1200, "ttl_by_mode_model_s": table()}));
}

#[tokio::test]
async fn post_applies_the_table_live() {
  let siren = SirenServer::start(Some(json!({"ttl_by_mode_model_s": table()}))).await;
  assert_eq!(siren.engine().config().ttl_for("codex", "gpt-5.6-sol"), Some(1800.0));

  let update = json!({"ttl_s": 900, "ttl_by_mode_model_s": {"codex": {"gpt-5.6": 600}}});
  let (status, body) = siren.post("/ttl", &update).await;

  assert_eq!(status, 200);
  assert_eq!((&body["ok"], &body["persisted"]), (&json!(true), &json!(true)));
  let config = siren.engine().config();
  assert_eq!(config.ttl_for("codex", "gpt-5.6-sol"), Some(600.0));
  assert_eq!(config.ttl_for("unlisted", "some-model"), Some(900.0));
  let cold = Request {
    kind: Kind::Cold,
    flags: Flags::default(),
    mode: "codex",
    model: "gpt-5.6-sol",
    last_model: Some("gpt-5.6-sol"),
    gap_s: Some(601.0),
    read: 0,
    prev_ctx: 0,
  };
  assert_eq!(config.attribute(&cold), Some(Cause::TtlExpired));
  assert_eq!(siren.get("/ttl").await.1, update);
}

#[tokio::test]
async fn post_merges_only_the_ttl_keys_into_the_config_file() {
  let written = json!({"port": 9022, "voice_cmd": ["/opt/tts", "{text}"], "ttl_s": 3600, "ttl_by_mode_model_s": table()});
  let siren = SirenServer::start(Some(written.clone())).await;

  siren.post("/ttl", &json!({"ttl_by_mode_model_s": {"claude-code": {"*": 300}}})).await;

  let mut expected = written;
  expected["ttl_by_mode_model_s"] = json!({"claude-code": {"*": 300}});
  assert_eq!(siren.config_file(), expected);
  assert!(no_temporary_in(siren.config_path.parent().unwrap()));
}

#[tokio::test]
async fn post_writes_through_a_symlinked_config_to_its_target() {
  let siren = SirenServer::start(Some(json!({"ttl_s": 3600}))).await;
  let elsewhere = tempfile::tempdir().unwrap();
  let target = elsewhere.path().join("config.json");
  fs::write(&target, json!({"port": 9022, "ttl_s": 3600}).to_string()).unwrap();
  fs::remove_file(&siren.config_path).unwrap();
  std::os::unix::fs::symlink(&target, &siren.config_path).unwrap();

  siren.post("/ttl", &json!({"ttl_s": 900})).await;

  assert!(fs::symlink_metadata(&siren.config_path).unwrap().file_type().is_symlink());
  assert_eq!(fs::read_link(&siren.config_path).unwrap(), target);
  let on_disk: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
  assert_eq!(on_disk, json!({"port": 9022, "ttl_s": 900}));
  for folder in [elsewhere.path(), siren.config_path.parent().unwrap()] {
    assert!(no_temporary_in(folder));
  }
}

#[tokio::test]
async fn a_restart_reads_back_what_was_posted() {
  let siren = SirenServer::start(Some(json!({"ttl_s": 3600}))).await;
  siren.post("/ttl", &json!({"ttl_s": 1500})).await;

  assert_eq!(read_at_start(&siren).ttl_s, 1500.into());
}

#[tokio::test]
async fn bad_bodies_are_refused_and_change_nothing() {
  let siren = SirenServer::start(Some(json!({"ttl_s": 3600, "ttl_by_mode_model_s": table()}))).await;
  let before_file = siren.config_file();
  let cases = [
    (json!([1, 2]), "body_not_object"),
    (json!({}), "nothing_to_set"),
    (json!({"ttl_s": 60, "port": 1}), "unknown_field"),
    (json!({"ttl_s": 0}), "ttl_s_not_positive_integer"),
    (json!({"ttl_s": -5}), "ttl_s_not_positive_integer"),
    (json!({"ttl_s": 1.5}), "ttl_s_not_positive_integer"),
    (json!({"ttl_s": true}), "ttl_s_not_positive_integer"),
    (json!({"ttl_s": "3600"}), "ttl_s_not_positive_integer"),
    (json!({"ttl_by_mode_model_s": [1]}), "ttl_table_not_object"),
    (json!({"ttl_by_mode_model_s": {"codex": 1800}}), "ttl_table_mode_invalid"),
    (json!({"ttl_by_mode_model_s": {"": {}}}), "ttl_table_mode_invalid"),
    (json!({"ttl_by_mode_model_s": {"codex": {"": 60}}}), "ttl_table_model_invalid"),
    (
      json!({"ttl_by_mode_model_s": {"codex": {"gpt": 0}}}),
      "ttl_table_seconds_not_positive_integer",
    ),
    (
      json!({"ttl_by_mode_model_s": {"codex": {"gpt": "60"}}}),
      "ttl_table_seconds_not_positive_integer",
    ),
    (
      json!({"ttl_s": 60, "ttl_by_mode_model_s": {"codex": {"gpt": false}}}),
      "ttl_table_seconds_not_positive_integer",
    ),
  ];

  for (body, code) in cases {
    assert_eq!(siren.post("/ttl", &body).await, (400, json!({"ok": false, "error": code})), "{body}");
  }

  assert_eq!(
    siren.post_raw("/ttl", b"{not json").await,
    (400, json!({"ok": false, "error": "body_not_json"}))
  );
  assert_eq!(siren.get("/ttl").await.1, json!({"ttl_s": 3600, "ttl_by_mode_model_s": table()}));
  assert_eq!(siren.config_file(), before_file);
}

#[tokio::test]
async fn an_empty_mode_table_is_accepted() {
  let siren = SirenServer::start(None).await;

  let (status, _) = siren.post("/ttl", &json!({"ttl_by_mode_model_s": {"openrouter": {}}})).await;

  assert_eq!(status, 200);
  assert_eq!(siren.engine().config().ttl_for("openrouter", "vendor/model"), None);
}

/// The file-plus-restart writer: it edits the TTL keys in the file, then the service starts and reads
/// them.
#[tokio::test]
async fn a_file_edited_before_start_is_what_runs() {
  let siren = SirenServer::start(None).await;
  let mut written = siren.config_file();
  written["ttl_s"] = json!(2400);
  written["ttl_by_mode_model_s"] = json!({"claude-code": {"*": 2400}});
  fs::write(&siren.config_path, written.to_string()).unwrap();

  let config = read_at_start(&siren);

  assert_eq!(config.ttl_for("claude-code", "claude-opus-5"), Some(2400.0));
  assert_eq!(siren.app.config_path(), Some(siren.config_path.as_path()));
}

#[tokio::test]
async fn a_missing_file_is_created_with_the_defaults() {
  let siren = SirenServer::start(None).await;

  assert_eq!(siren.config_file(), serde_json::to_value(SirenServer::defaults(&siren.dir)).unwrap());
}
