//! The routes no other contract test reaches: the pages and assets, the unknown paths, the body limit,
//! the overlay state, the health answer, the test siren and the stream.

mod siren_server;

use serde_json::{Value, json};
use siren_server::SirenServer;

#[tokio::test]
async fn pages_assets_and_unknown_paths_answer_as_documented() {
  let siren = SirenServer::start(None).await;
  let client = reqwest::Client::new();

  let page = client.get(siren.url("/")).send().await.unwrap();
  assert_eq!(page.status(), 200);
  assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
  assert_eq!(page.headers()["cache-control"], "no-store");
  assert_eq!(client.get(siren.url("/demo")).send().await.unwrap().status(), 200);

  assert_eq!(
    siren.get("/asset/logo.png").await,
    (404, json!({"ok": false, "error": "asset not allowed"}))
  );
  let (status, missing) = siren.get("/asset/siren_on.png").await;
  assert_eq!((status, &missing["ok"]), (500, &json!(false)));
  assert_eq!(missing["file"], json!(siren.dir.path().join("siren_on.png")));

  assert_eq!(siren.get("/nowhere").await, (404, json!({"ok": false, "error": "not found"})));
  assert_eq!(siren.post("/nowhere", &json!({})).await, (404, json!({"ok": false})));
  let too_large = vec![b' '; 65_537];
  assert_eq!(
    siren.post_raw("/events", &too_large).await,
    (413, json!({"ok": false, "error": "body too large"}))
  );
}

#[tokio::test]
async fn the_overlay_state_is_set_kept_and_reported() {
  let siren = SirenServer::start(None).await;

  let (status, ctl) = siren.post("/ctl", &json!({"muted": true, "page": "/demo", "other": 1})).await;

  assert_eq!(status, 200);
  assert_eq!(
    ctl,
    json!({"visible": true, "muted": true, "opacity": 0.94, "show_local": false, "page": "/demo"})
  );
  assert_eq!(siren.get("/ctl").await.1, ctl);
  let kept: Value =
    serde_json::from_slice(&std::fs::read(siren.dir.path().join("ctl.json")).unwrap()).unwrap();
  assert_eq!(kept, ctl);
  assert_eq!(siren.post("/test-siren", &json!({})).await, (200, json!({"ok": true, "muted": true})));
  let (_, health) = siren.get("/health").await;
  assert_eq!(
    (&health["ok"], &health["service"], &health["events"]),
    (&json!(true), &json!("cache-siren"), &json!(0))
  );
}

#[tokio::test]
async fn the_stream_sends_a_new_row_and_the_stats() {
  let siren = SirenServer::start(None).await;
  let mut stream = reqwest::Client::new().get(siren.url("/stream")).send().await.unwrap();
  assert_eq!(stream.headers()["content-type"], "text/event-stream; charset=utf-8");

  let posted = json!({"mode": "codex", "model": "gpt-5.6-sol", "key": "worker", "usage_known": true});
  siren.post("/events", &posted).await;

  let mut seen = String::new();
  while !(seen.contains("event: ev") && seen.contains("event: stats")) {
    let chunk = stream.chunk().await.unwrap().expect("the stream stays open");
    seen.push_str(&String::from_utf8_lossy(&chunk));
  }
  assert!(seen.contains("\"model\":\"gpt-5.6-sol\""), "{seen}");
}
