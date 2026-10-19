//! The HTTP routes (docs/api.md, Served routes): the pages, the assets, the reads, the stream and the
//! four posts, over one engine. Every JSON answer is UTF-8 with `Cache-Control: no-store`.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::{Map, Value, json};

use crate::attribution::Kind;
use crate::config::{persist_config, ttl_body};
use crate::engine::{Alert, Engine, Row};

/// The largest POST body taken.
pub const MAX_BODY_BYTES: usize = 65_536;
/// The two overlay images a host may place in the data directory.
const ASSETS: [&str; 2] = ["siren_on.png", "siren_off.png"];
/// How long the stream waits for a new row before looking again.
const STREAM_WAIT: Duration = Duration::from_secs(1);
/// How often the stream sends the stats object.
const STREAM_STATS_EVERY: Duration = Duration::from_secs(2);

/// What the routes serve from: the engine, the pages and the config file `POST /ttl` merges into.
pub struct App {
  engine: Arc<Engine>,
  web_dir: PathBuf,
  config_path: Option<PathBuf>,
  /// Serialises TTL updates, so the live table and the file agree on the last one.
  ttl_lock: Mutex<()>,
}

impl App {
  pub fn new(engine: Arc<Engine>, web_dir: impl Into<PathBuf>, config_path: Option<PathBuf>) -> App {
    App {
      engine,
      web_dir: web_dir.into(),
      config_path,
      ttl_lock: Mutex::new(()),
    }
  }

  pub fn engine(&self) -> &Arc<Engine> {
    &self.engine
  }

  pub fn config_path(&self) -> Option<&Path> {
    self.config_path.as_deref()
  }
}

type Shared = State<Arc<App>>;

pub fn router(app: Arc<App>) -> Router {
  Router::new()
    .route("/", get(index))
    .route("/index.html", get(index))
    .route("/demo", get(demo))
    .route("/asset/{*name}", get(asset))
    .route("/health", get(health))
    .route("/stats", get(stats))
    .route("/events", get(events).post(post_events))
    .route("/stream", get(stream_rows))
    .route("/ctl", get(ctl).post(post_ctl))
    .route("/ttl", get(ttl).post(post_ttl))
    .route("/test-siren", axum::routing::post(test_siren))
    .fallback(unknown)
    .with_state(app)
}

/// A JSON answer.
fn json_answer(status: StatusCode, body: Value) -> Response {
  let mut response = (status, body.to_string()).into_response();
  let headers = response.headers_mut();
  headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
  headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
  response
}

/// A file as it is on disk; 500 with the reason and the path tried when it cannot be read.
async fn file_answer(path: PathBuf, content_type: &'static str) -> Response {
  match tokio::fs::read(&path).await {
    Ok(body) => {
      let headers = [(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-store")];
      (headers, body).into_response()
    }
    Err(why) => json_answer(
      StatusCode::INTERNAL_SERVER_ERROR,
      json!({"ok": false, "error": why.to_string(), "file": path}),
    ),
  }
}

async fn index(State(app): Shared) -> Response {
  file_answer(app.web_dir.join("index.html"), "text/html; charset=utf-8").await
}

async fn demo(State(app): Shared) -> Response {
  file_answer(app.web_dir.join("demo.html"), "text/html; charset=utf-8").await
}

async fn asset(State(app): Shared, uri: Uri) -> Response {
  let name = uri.path().rsplit('/').next().unwrap_or("");
  if !ASSETS.contains(&name) {
    return json_answer(StatusCode::NOT_FOUND, json!({"ok": false, "error": "asset not allowed"}));
  }
  file_answer(app.engine.data_dir().root().join(name), "image/png").await
}

async fn health(State(app): Shared) -> Response {
  let stats = app.engine.stats();
  json_answer(
    StatusCode::OK,
    json!({"ok": true, "service": "cache-siren", "uptime_s": stats["uptime_s"], "files": stats["files"],
           "last_event": stats["last_event"], "events": app.engine.rows().len(), "quota": stats["quota"]}),
  )
}

async fn stats(State(app): Shared) -> Response {
  json_answer(StatusCode::OK, app.engine.stats())
}

async fn ctl(State(app): Shared) -> Response {
  json_answer(StatusCode::OK, json!(app.engine.ctl()))
}

async fn ttl(State(app): Shared) -> Response {
  json_answer(StatusCode::OK, app.engine.config().ttl())
}

/// An integer query parameter; its default where it is missing or no integer.
fn number(query: &HashMap<String, String>, name: &str, default: i64) -> i64 {
  query.get(name).and_then(|value| value.parse().ok()).unwrap_or(default)
}

fn seq_of(row: &Row) -> u64 {
  row.get("seq").and_then(Value::as_u64).unwrap_or(0)
}

fn is_local(row: &Row) -> bool {
  row.get("backend").and_then(Value::as_str) == Some("local")
}

async fn events(State(app): Shared, Query(query): Query<HashMap<String, String>>) -> Response {
  let limit = number(&query, "limit", 80);
  let since = number(&query, "since", 0);
  let show_local = query.get("local").map(String::as_str) == Some("1");
  let seq = app.engine.seq();
  let rows: Vec<Row> = app
    .engine
    .rows()
    .into_iter()
    .filter(|row| seq_of(row) as i64 > since && (show_local || !is_local(row)))
    .collect();
  // The newest `limit` rows; 0 answers all of them.
  let skip = if limit > 0 { rows.len().saturating_sub(limit as usize) } else { 0 };
  let rows: Vec<Row> = rows.into_iter().skip(skip).collect();
  json_answer(StatusCode::OK, json!({"events": rows, "seq": seq}))
}

/// What the stream holds between two looks: the last row sent and when the stats last went.
struct Cursor {
  app: Arc<App>,
  since: u64,
  stats_at: Option<Instant>,
  changes: tokio::sync::watch::Receiver<u64>,
}

async fn stream_rows(
  State(app): Shared,
  Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
  let since = query
    .get("since")
    .and_then(|value| value.parse().ok())
    .unwrap_or_else(|| app.engine.seq());
  let changes = app.engine.subscribe();
  let cursor = Cursor {
    app,
    since,
    stats_at: None,
    changes,
  };
  let headers = [
    (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
    (header::CACHE_CONTROL, "no-store"),
  ];
  (headers, Sse::new(feed(cursor)))
}

/// Every row after the cursor as an `ev` event as it arrives, and the stats object about every two
/// seconds, until the client leaves.
fn feed(cursor: Cursor) -> impl Stream<Item = Result<SseEvent, Infallible>> {
  stream::unfold(cursor, |mut cursor| async move {
    let _ = tokio::time::timeout(STREAM_WAIT, cursor.changes.changed()).await;
    let mut out = Vec::new();
    for row in cursor.app.engine.rows() {
      let seq = seq_of(&row);
      if seq > cursor.since {
        cursor.since = seq;
        out.push(Ok(SseEvent::default().event("ev").data(Value::Object(row).to_string())));
      }
    }
    if cursor.stats_at.is_none_or(|at| at.elapsed() > STREAM_STATS_EVERY) {
      cursor.stats_at = Some(Instant::now());
      out.push(Ok(SseEvent::default().event("stats").data(cursor.app.engine.stats().to_string())));
    }
    Some((stream::iter(out), cursor))
  })
  .flatten()
}

/// A POST body up to `MAX_BODY_BYTES`; none when it is larger.
async fn read_body(headers: &HeaderMap, body: Body) -> Option<Bytes> {
  let declared = headers
    .get(header::CONTENT_LENGTH)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.parse::<usize>().ok());
  if declared.is_some_and(|length| length > MAX_BODY_BYTES) {
    return None;
  }
  to_bytes(body, MAX_BODY_BYTES).await.ok()
}

fn too_large() -> Response {
  json_answer(StatusCode::PAYLOAD_TOO_LARGE, json!({"ok": false, "error": "body too large"}))
}

/// A body as JSON: an empty body is an empty object; none when it is not JSON.
fn parse(raw: &[u8]) -> Option<Value> {
  if raw.is_empty() {
    return Some(Value::Object(Map::new()));
  }
  serde_json::from_slice(raw).ok()
}

async fn post_ttl(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  let Some(raw) = read_body(&headers, body).await else { return too_large() };
  let Some(body) = parse(&raw) else {
    return json_answer(StatusCode::BAD_REQUEST, json!({"ok": false, "error": "body_not_json"}));
  };
  let update = match ttl_body(&body) {
    Ok(update) => update,
    Err(refused) => {
      return json_answer(StatusCode::BAD_REQUEST, json!({"ok": false, "error": refused.code()}));
    }
  };
  let engine = &app.engine;
  let _held = app.ttl_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let (before, after) = engine.with_config(|config| {
    let before = config.ttl();
    update.apply(config);
    (before, config.ttl())
  });
  engine
    .log()
    .info("ttl set", json!({"before": before, "after": after, "config": app.config_path}));
  let persisted = match &app.config_path {
    None => false,
    Some(path) => match persist_config(path, &update.to_map()) {
      Ok(()) => true,
      Err(why) => {
        engine
          .log()
          .error("ttl persist failed", json!({"err": why.to_string(), "config": path}));
        let mut answer = json!({"ok": false, "error": "config_write_failed", "applied": true});
        merge(&mut answer, after);
        return json_answer(StatusCode::INTERNAL_SERVER_ERROR, answer);
      }
    },
  };
  let mut answer = json!({"ok": true, "persisted": persisted});
  merge(&mut answer, after);
  json_answer(StatusCode::OK, answer)
}

/// Add the keys of `extra` to the object `into`.
fn merge(into: &mut Value, extra: Value) {
  if let (Value::Object(into), Value::Object(extra)) = (into, extra) {
    into.extend(extra);
  }
}

async fn post_events(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  let Some(raw) = read_body(&headers, body).await else { return too_large() };
  let body = parse(&raw).unwrap_or_else(|| Value::Object(Map::new()));
  match app.engine.external(&body) {
    Ok(event) => {
      let seq = app.engine.emit(event, false);
      json_answer(StatusCode::OK, json!({"ok": true, "seq": seq}))
    }
    Err(why) => json_answer(StatusCode::BAD_REQUEST, json!({"ok": false, "error": why})),
  }
}

async fn post_ctl(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  let Some(raw) = read_body(&headers, body).await else { return too_large() };
  let body = match parse(&raw) {
    Some(Value::Object(body)) => body,
    _ => Map::new(),
  };
  let (before, after) = app.engine.set_ctl(&body);
  app.engine.log().info("ctl", json!({"before": before, "after": after}));
  json_answer(StatusCode::OK, json!(after))
}

async fn test_siren(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  if read_body(&headers, body).await.is_none() {
    return too_large();
  }
  let muted = app.engine.alert(&Alert {
    kind: Kind::Cold,
    project: "test".into(),
    sid: "test".into(),
    new: 123_456,
    note: "manual test".into(),
    force: true,
  });
  json_answer(StatusCode::OK, json!({"ok": true, "muted": muted}))
}

async fn unknown(method: Method, headers: HeaderMap, body: Body) -> Response {
  if method == Method::GET {
    return json_answer(StatusCode::NOT_FOUND, json!({"ok": false, "error": "not found"}));
  }
  if read_body(&headers, body).await.is_none() {
    return too_large();
  }
  json_answer(StatusCode::NOT_FOUND, json!({"ok": false}))
}
