//! The HTTP API as docs/api.md states it, over a real server: the token, the error shape, health, the
//! events as a page and as a stream, the stats, the settings, the siren tests and the pages.

use crate::siren_server;

use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};
use siren_server::{SirenServer, TOKEN};

/// A posted usage of `session`, with `new` new tokens and `cached` read from the cache.
fn usage(session: &str, new: u64, cached: u64) -> Value {
  json!({
    "sessionId": session, "modelName": "gpt-5.6-sol", "harnessName": "codex",
    "newTokenCount": new, "cachedTokenCount": cached, "at": 1_700_000_000_000_i64,
    "outputTokenCount": 30, "scope": "worker", "actorName": "runner-7", "labels": {"queue": "build"},
  })
}

fn ids(page: &Value) -> Vec<i64> {
  page["items"]
    .as_array()
    .unwrap()
    .iter()
    .map(|event| event["id"].as_i64().unwrap())
    .collect()
}

#[tokio::test]
async fn every_route_takes_the_token_and_the_pages_take_none() {
  let siren = SirenServer::start(None).await;
  let http = reqwest::Client::new();

  let bare = http.get(siren.url("/health")).send().await.unwrap();
  assert_eq!(bare.status(), 401);
  assert_eq!(bare.json::<Value>().await.unwrap()["code"], "unauthorized");
  let wrong = http.get(siren.url("/settings")).bearer_auth("other").send().await.unwrap();
  assert_eq!(wrong.status(), 401);
  let by_query = http.get(siren.url(&format!("/settings?access_token={TOKEN}"))).send().await.unwrap();
  assert_eq!(by_query.status(), 200, "a reader that cannot set a header gives it in the query");

  let page = http.get(siren.url("/")).send().await.unwrap();
  assert_eq!(page.status(), 200);
  assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
  assert_eq!(http.get(siren.url("/demo")).send().await.unwrap().status(), 200);
  let missing = http.get(siren.url("/asset/siren_on.png")).send().await.unwrap();
  assert_eq!(missing.status(), 404);
  let missing: Value = missing.json().await.unwrap();
  assert_eq!(missing, json!({"code": "not-found", "error": "There is no asset siren_on.png."}));
  std::fs::write(siren.dir.path().join("siren_off.png"), b"png").unwrap();
  let placed = http.get(siren.url("/asset/siren_off.png")).send().await.unwrap();
  assert_eq!(
    (placed.status().as_u16(), placed.bytes().await.unwrap().as_ref()),
    (200, b"png".as_ref())
  );
  assert_eq!(http.get(siren.url("/asset/logo.png")).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn failures_answer_in_the_common_error_shape() {
  let siren = SirenServer::start(None).await;

  let (status, body) = siren.get("/nowhere").await;
  assert_eq!((status, body["code"].as_str()), (404, Some("no-route")));
  let (status, body) = siren.call(Method::DELETE, "/settings", None).await;
  assert_eq!((status, body["code"].as_str()), (404, Some("no-route")), "a method no route answers");
  let too_large = siren.request(Method::POST, "/events").body(vec![b' '; 65_537]).send().await.unwrap();
  assert_eq!(too_large.status(), 413);
  assert_eq!(too_large.json::<Value>().await.unwrap()["code"], "too-large");
  let (status, body) = siren.get("/events?after=many").await;
  assert_eq!((status, body["code"].as_str()), (400, Some("bad-request")));
  assert_eq!(body.as_object().unwrap().len(), 2, "exactly code and error: {body}");

  let (status, health) = siren.get("/health").await;
  assert_eq!(status, 200);
  assert_eq!(health, json!({"ready": true, "reason": null, "version": cache_siren::VERSION}));
}

#[tokio::test]
async fn a_posted_usage_is_an_event_and_the_page_reads_it() {
  let siren = SirenServer::start(None).await;

  let (status, event) = siren.post("/events", &usage("thread-1", 120, 4000)).await;
  assert_eq!(status, 201);
  assert_eq!((&event["id"], &event["kind"]), (&json!(1), &json!("usage.created")));
  let data = &event["data"];
  assert_eq!(
    (&data["sessionId"], &data["kind"], &data["backend"]),
    (&json!("thread-1"), &json!("warm"), &json!("openai")),
    "a first request that reads from the cache is warm"
  );
  assert_eq!(
    (&data["scope"], &data["actorName"], &data["projectName"]),
    (&json!("worker"), &json!("runner-7"), &json!("worker"))
  );
  assert_eq!(
    (&data["labels"], &data["at"]),
    (&json!({"queue": "build"}), &json!(1_700_000_000_000_i64))
  );

  let mut unknown = usage("thread-1", 1, 1);
  unknown["agent"] = json!("job-7");
  let (status, body) = siren.post("/events", &unknown).await;
  assert_eq!((status, body["code"].as_str()), (400, Some("bad-request")));
  assert!(body["error"].as_str().unwrap().contains("agent"), "{body}");
  let (status, body) = siren.post("/events", &usage(" ", 1, 1)).await;
  assert_eq!((status, body["error"].as_str()), (400, Some("sessionId must not be blank")));

  for session in ["thread-2", "thread-3", "thread-2"] {
    siren.post("/events", &usage(session, 10, 10)).await;
  }
  let (_, newest) = siren.get("/events?limit=2").await;
  assert_eq!((ids(&newest), &newest["more"]), (vec![3, 4], &json!(false)), "the newest, oldest first");
  let (_, after) = siren.get("/events?after=1&limit=2").await;
  assert_eq!((ids(&after), &after["more"]), (vec![2, 3], &json!(true)));
  let (_, session) = siren.get("/events?sessionId=thread-2").await;
  assert_eq!(ids(&session), [2, 4]);
  let (_, kinds) = siren.get("/events?kinds=settings.changed").await;
  assert_eq!(ids(&kinds), Vec::<i64>::new());
  let (status, body) = siren.get("/events?kinds=usage.created,run.stopped").await;
  assert_eq!((status, body["code"].as_str()), (400, Some("bad-request")));
  let (status, _) = siren.get("/events?limit=0").await;
  assert_eq!(status, 400);
}

#[tokio::test]
async fn a_resume_from_before_the_window_is_gone() {
  let siren = SirenServer::start(Some(json!({"ring": 2}))).await;
  for session in ["a", "b", "c", "d"] {
    siren.post("/events", &usage(session, 1, 1)).await;
  }

  let (_, window) = siren.get("/events").await;
  assert_eq!(ids(&window), [3, 4]);
  assert_eq!(siren.get("/events?after=2").await.0, 200, "the event just before the window");
  let (status, body) = siren.get("/events?after=1").await;
  assert_eq!((status, body["code"].as_str()), (410, Some("gone")));
  let stream = reqwest::Client::new()
    .get(siren.url(&format!("/events?after=0&access_token={TOKEN}")))
    .send()
    .await
    .unwrap();
  assert_eq!(stream.status(), 410, "the stream refuses it too");
}

/// Read a stream until `seen` holds every one of `wanted`, for at most two seconds of waiting on it.
async fn read_until(stream: &mut reqwest::Response, wanted: &[&str]) -> String {
  let mut seen = String::new();
  while !wanted.iter().all(|want| seen.contains(want)) {
    let chunk = tokio::time::timeout(Duration::from_secs(2), stream.chunk()).await;
    let chunk = chunk
      .unwrap_or_else(|_| panic!("no {wanted:?} in {seen}"))
      .unwrap()
      .expect("the stream stays open");
    seen.push_str(&String::from_utf8_lossy(&chunk));
  }
  seen
}

#[tokio::test]
async fn a_stop_ends_every_open_stream() {
  let siren = SirenServer::start(None).await;
  let mut stream = reqwest::Client::new()
    .get(siren.url("/events"))
    .bearer_auth(TOKEN)
    .send()
    .await
    .unwrap();
  assert_eq!(stream.status(), 200);

  siren.app.stop();
  while let Some(chunk) = stream.chunk().await.expect("the stream ends cleanly") {
    assert!(!String::from_utf8_lossy(&chunk).contains("event:"), "nothing was recorded");
  }
}

#[tokio::test]
async fn the_stream_sends_each_event_and_resumes_after_the_last_one_read() {
  let siren = SirenServer::start(None).await;
  siren.post("/events", &usage("before", 1, 1)).await;
  let http = reqwest::Client::new();

  let mut stream = http.get(siren.url("/events")).bearer_auth(TOKEN).send().await.unwrap();
  assert_eq!(stream.headers()["content-type"], "text/event-stream");
  siren.patch("/settings", &json!({"armed": false})).await;
  let seen = read_until(&mut stream, &["event: settings.changed", "id: 2"]).await;
  assert!(
    !seen.contains("\"before\""),
    "a stream with no cursor starts with the next event: {seen}"
  );
  assert!(seen.contains("\"data\":{\"armed\":false"), "{seen}");

  let mut resumed = http
    .get(siren.url("/events?kinds=usage.created"))
    .bearer_auth(TOKEN)
    .header("Last-Event-ID", "0")
    .send()
    .await
    .unwrap();
  let seen = read_until(&mut resumed, &["id: 1"]).await;
  assert!(seen.contains("event: usage.created") && !seen.contains("settings.changed"), "{seen}");
}

#[tokio::test]
async fn stats_sum_each_session_since_a_time() {
  let siren = SirenServer::start(None).await;
  let mut early = usage("thread-1", 100, 300);
  early["at"] = json!(1000);
  siren.post("/events", &early).await;
  let mut cold = usage("thread-1", 150_000, 0);
  cold["at"] = json!(2000);
  siren.post("/events", &cold).await;
  let mut other = usage("thread-2", 50, 50);
  other["at"] = json!(3000);
  siren.post("/events", &other).await;

  let (status, stats) = siren.get("/stats?since=0").await;
  assert_eq!(status, 200);
  assert_eq!(
    (&stats["since"], &stats["sirenCount"]),
    (&json!(0), &json!(1)),
    "the red cold request raised one"
  );
  let sessions = stats["sessions"].as_array().unwrap();
  assert_eq!(
    sessions.iter().map(|session| &session["sessionId"]).collect::<Vec<_>>(),
    [&json!("thread-2"), &json!("thread-1")]
  );
  let first = &sessions[1];
  assert_eq!(
    (&first["requestCount"], &first["newTokenCount"], &first["cachedTokenCount"]),
    (&json!(2), &json!(150_100), &json!(300))
  );
  assert_eq!(
    (&first["coldCount"], &first["firstAt"], &first["lastAt"]),
    (&json!(1), &json!(1000), &json!(2000))
  );
  assert_eq!(
    first["cachedShare"].as_f64().map(|share| (share * 1e4).round()),
    Some((300.0 / 150_400.0 * 1e4_f64).round())
  );

  let (_, later) = siren.get("/stats?since=1500&sessionId=thread-1").await;
  assert_eq!(later["sessions"].as_array().unwrap().len(), 1);
  assert_eq!(later["sessions"][0]["requestCount"], json!(1));
  assert_eq!(later["sirenCount"], json!(1));
  let (_, by_default) = siren.get("/stats").await;
  assert_eq!(by_default["since"], json!(1000), "the oldest event held");
  assert_eq!(siren.get("/stats?since=-1").await.0, 400);
}

#[tokio::test]
async fn settings_change_live_are_kept_in_the_config_file_and_recorded() {
  let siren = SirenServer::start(None).await;
  assert_eq!(
    siren.get("/settings").await,
    (200, json!({"armed": true, "ttlMs": 3_600_000, "threshold": 100_000}))
  );

  let (status, changed) = siren
    .patch("/settings", &json!({"armed": false, "ttlMs": 1500, "threshold": null}))
    .await;
  assert_eq!((status, changed), (200, json!({"armed": false, "ttlMs": 1500, "threshold": 100_000})));
  let file = siren.config_file();
  assert_eq!(
    (&file["armed"], &file["ttl_s"], &file["red_new_tokens"]),
    (&json!(false), &json!(1.5), &json!(100_000))
  );
  assert_eq!(
    siren.engine().config().ttl_for("unlisted", "m"),
    Some(1.5),
    "the lifetime applies at once"
  );

  siren.patch("/settings", &json!({})).await;
  let (_, recorded) = siren.get("/events?kinds=settings.changed").await;
  assert_eq!(recorded["items"].as_array().unwrap().len(), 1, "an unchanged patch records nothing");

  let (status, body) = siren.patch("/settings", &json!({"threshold": 20_000})).await;
  assert_eq!((status, body["code"].as_str()), (409, Some("conflict")), "not above the yellow line");
  let (status, body) = siren.patch("/settings", &json!({"ttlMs": 0})).await;
  assert_eq!((status, body["code"].as_str()), (400, Some("bad-request")));
  let (status, _) = siren.patch("/settings", &json!({"muted": true})).await;
  assert_eq!(status, 400);
  assert_eq!(siren.get("/settings").await.1["threshold"], json!(100_000));
}

#[tokio::test]
async fn a_siren_test_sounds_once_and_keeps_nothing() {
  let siren = SirenServer::start(None).await;

  let (status, test) = siren.post("/siren-tests", &json!({})).await;

  assert_eq!((status, &test["sounded"]), (200, &json!(false)), "the test server sounds nothing");
  assert!(test["at"].as_i64().is_some_and(|at| at > 0));
  assert_eq!(siren.get("/events").await.1["items"], json!([]));
}
