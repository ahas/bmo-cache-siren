//! The HTTP API and the pages (docs/api.md): health, the events and their stream, the stats, the settings
//! and the siren tests behind the bearer token, and the pages and their assets outside it. Every JSON
//! answer is UTF-8 with `Cache-Control: no-store`, and every failure is the common error shape.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::watch;

use crate::clock::now_millis;
use crate::config::persist_config;
use crate::engine::{Engine, Filter, Refusal};
use crate::wire::{Event, EventQuery, Health, NewUsage, SettingsPatch, SirenTest, StatsQuery, WireError};

/// The largest body taken.
pub const MAX_BODY_BYTES: usize = 65_536;
/// The two page images a host may place in the data directory.
const ASSETS: [&str; 2] = ["siren_on.png", "siren_off.png"];
/// How often a comment line keeps the stream open.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// What the routes serve from: the engine, the pages, the config file the settings merge into and the
/// token every API route takes.
pub struct App {
  engine: Arc<Engine>,
  web_dir: PathBuf,
  config_path: Option<PathBuf>,
  token: String,
  /// Serialises settings changes, so the live settings and the file agree on the last one.
  settings_lock: Mutex<()>,
  /// Turns true once the daemon stops, which ends every open stream.
  stopping: watch::Sender<bool>,
}

impl App {
  pub fn new(
    engine: Arc<Engine>,
    web_dir: impl Into<PathBuf>,
    config_path: Option<PathBuf>,
    token: impl Into<String>,
  ) -> App {
    App {
      engine,
      web_dir: web_dir.into(),
      config_path,
      token: token.into(),
      settings_lock: Mutex::new(()),
      stopping: watch::Sender::new(false),
    }
  }

  /// Ends every open stream, so a graceful stop waits only for the answers under way.
  pub fn stop(&self) {
    self.stopping.send_replace(true);
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
  let api = Router::new()
    .route("/health", get(health))
    .route("/events", get(events).post(create_usage))
    .route("/stats", get(stats))
    .route("/settings", get(settings).patch(change_settings))
    .route("/siren-tests", post(siren_test))
    .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
  let pages = Router::new()
    .route("/", get(index))
    .route("/demo", get(demo))
    .route("/asset/{name}", get(asset));
  api
    .merge(pages)
    .fallback(no_route)
    .method_not_allowed_fallback(no_route)
    .with_state(app)
}

/// A JSON answer.
fn json_answer(status: StatusCode, body: &impl Serialize) -> Response {
  let text = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
  let mut response = (status, text).into_response();
  let headers = response.headers_mut();
  headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
  headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
  response
}

/// A failure in the common error shape.
fn refused(status: StatusCode, code: &str, error: impl Into<String>) -> Response {
  json_answer(
    status,
    &WireError {
      code: code.into(),
      error: error.into(),
    },
  )
}

fn bad_request(error: impl Into<String>) -> Response {
  refused(StatusCode::BAD_REQUEST, "bad-request", error)
}

fn refusal(refusal: Refusal) -> Response {
  match refusal {
    Refusal::BadRequest(why) => bad_request(why),
    Refusal::Conflict(why) => refused(StatusCode::CONFLICT, "conflict", why),
    Refusal::Gone(why) => refused(StatusCode::GONE, "gone", why),
  }
}

async fn no_route() -> Response {
  refused(StatusCode::NOT_FOUND, "no-route", "No route answers this method and path.")
}

fn too_large() -> Response {
  refused(
    StatusCode::PAYLOAD_TOO_LARGE,
    "too-large",
    format!("The body is over {MAX_BODY_BYTES} bytes."),
  )
}

/// The token a reader that cannot set a header gives in the query (RFC 6750, section 2.3).
#[derive(Deserialize)]
struct TokenQuery {
  access_token: Option<String>,
}

/// Every API request is let in only with the service's token, as its bearer or as `access_token`.
async fn authorize(State(app): Shared, request: Request, next: Next) -> Response {
  let bearer = request
    .headers()
    .get(header::AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "))
    .map(str::to_string);
  let given = bearer.or_else(|| {
    Query::<TokenQuery>::try_from_uri(request.uri())
      .ok()
      .and_then(|query| query.0.access_token)
  });
  match given {
    Some(token) if token == app.token => next.run(request).await,
    Some(_) => refused(StatusCode::UNAUTHORIZED, "unauthorized", "The bearer token is not the service's."),
    None => refused(StatusCode::UNAUTHORIZED, "unauthorized", "The request carries no bearer token."),
  }
}

/// A body up to `MAX_BODY_BYTES`; none when it is larger.
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

async fn health() -> Response {
  json_answer(
    StatusCode::OK,
    &Health {
      ready: true,
      reason: None,
      version: env!("CARGO_PKG_VERSION").into(),
    },
  )
}

/// The stream by default, the page to a reader asking for JSON (docs/api.md, `GET /events`).
async fn events(
  State(app): Shared,
  headers: HeaderMap,
  query: Result<Query<EventQuery>, QueryRejection>,
) -> Response {
  let mut query = match query {
    Ok(Query(query)) => query,
    Err(rejection) => return bad_request(rejection.body_text()),
  };
  let wants_page = headers
    .get(header::ACCEPT)
    .and_then(|value| value.to_str().ok())
    .is_some_and(|accept| accept.contains("application/json"));
  if wants_page {
    return match app.engine.page(&query) {
      Ok(page) => json_answer(StatusCode::OK, &page),
      Err(why) => refusal(why),
    };
  }
  if query.after.is_none() {
    let last = headers.get("last-event-id").and_then(|value| value.to_str().ok());
    match last.map(|last| last.trim().parse::<i64>()) {
      Some(Ok(last)) => query.after = Some(last),
      Some(Err(_)) => return bad_request("Last-Event-ID must be an event number"),
      None => {}
    }
  }
  let filter = match Filter::of(&query) {
    Ok(filter) => filter,
    Err(why) => return refusal(why),
  };
  let start = match app.engine.stream_start(query.after) {
    Ok(start) => start,
    Err(why) => return refusal(why),
  };
  let reader = Reader {
    changes: app.engine.subscribe(),
    stopping: app.stopping.subscribe(),
    app,
    cursor: start,
    filter,
  };
  let mut response =
    Sse::new(feed(reader)).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)).into_response();
  response
    .headers_mut()
    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
  response
}

/// What the stream holds between two looks: the last event sent and what it lets through.
struct Reader {
  app: Arc<App>,
  cursor: i64,
  filter: Filter,
  changes: watch::Receiver<i64>,
  stopping: watch::Receiver<bool>,
}

/// Every event past the cursor that the filter lets through, as it is recorded, until the reader leaves or
/// the daemon stops.
fn feed(reader: Reader) -> impl Stream<Item = Result<SseEvent, Infallible>> {
  stream::unfold(reader, |mut reader| async move {
    loop {
      let events = reader.app.engine.events_after(reader.cursor, &reader.filter);
      if let Some(last) = events.last() {
        reader.cursor = last.id;
        let sent: Vec<Result<SseEvent, Infallible>> =
          events.iter().map(|event| Ok(sse_of(event))).collect();
        return Some((stream::iter(sent), reader));
      }
      tokio::select! {
        changed = reader.changes.changed() => {
          if changed.is_err() {
            return None;
          }
        }
        _ = reader.stopping.wait_for(|stopping| *stopping) => return None,
      }
    }
  })
  .flatten()
}

fn sse_of(event: &Event) -> SseEvent {
  SseEvent::default()
    .event(event.happening.kind())
    .id(event.id.to_string())
    .data(serde_json::to_string(event).unwrap_or_default())
}

async fn create_usage(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  let Some(raw) = read_body(&headers, body).await else { return too_large() };
  let body: NewUsage = match serde_json::from_slice(&raw) {
    Ok(body) => body,
    Err(why) => return bad_request(format!("The body is not a usage: {why}.")),
  };
  match app.engine.posted(body) {
    Ok(usage) => json_answer(StatusCode::CREATED, &app.engine.record_usage(usage, false)),
    Err(why) => bad_request(why),
  }
}

async fn stats(State(app): Shared, query: Result<Query<StatsQuery>, QueryRejection>) -> Response {
  let query = match query {
    Ok(Query(query)) => query,
    Err(rejection) => return bad_request(rejection.body_text()),
  };
  match app.engine.stats(&query) {
    Ok(stats) => json_answer(StatusCode::OK, &stats),
    Err(why) => refusal(why),
  }
}

async fn settings(State(app): Shared) -> Response {
  json_answer(StatusCode::OK, &app.engine.settings())
}

async fn change_settings(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  let Some(raw) = read_body(&headers, body).await else { return too_large() };
  let patch: SettingsPatch = match serde_json::from_slice(&raw) {
    Ok(patch) => patch,
    Err(why) => return bad_request(format!("The body is not a settings patch: {why}.")),
  };
  let _held = app.settings_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let (settings, changed) = match app.engine.change_settings(&patch) {
    Ok(changed) => changed,
    Err(why) => return refusal(why),
  };
  if changed.is_empty() {
    return json_answer(StatusCode::OK, &settings);
  }
  let engine = &app.engine;
  engine
    .log()
    .info("settings changed", serde_json::json!({"changed": changed, "config": app.config_path}));
  if let Some(path) = &app.config_path
    && let Err(why) = persist_config(path, &changed)
  {
    engine
      .log()
      .error("settings persist failed", serde_json::json!({"err": why.to_string(), "config": path}));
    return refused(
      StatusCode::INTERNAL_SERVER_ERROR,
      "failed",
      format!("The settings changed and hold now, but the config file could not be written: {why}."),
    );
  }
  json_answer(StatusCode::OK, &settings)
}

async fn siren_test(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
  if read_body(&headers, body).await.is_none() {
    return too_large();
  }
  let sounded = app.engine.test_siren();
  json_answer(
    StatusCode::OK,
    &SirenTest {
      sounded,
      at: now_millis(),
    },
  )
}

/// A page as it is on disk.
async fn page(app: &App, name: &str) -> Response {
  match tokio::fs::read(app.web_dir.join(name)).await {
    Ok(body) => {
      let headers =
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")];
      (headers, body).into_response()
    }
    Err(why) => refused(
      StatusCode::INTERNAL_SERVER_ERROR,
      "failed",
      format!("The page {name} cannot be read: {}.", why.kind()),
    ),
  }
}

async fn index(State(app): Shared) -> Response {
  page(&app, "index.html").await
}

async fn demo(State(app): Shared) -> Response {
  page(&app, "demo.html").await
}

async fn asset(State(app): Shared, axum::extract::Path(name): axum::extract::Path<String>) -> Response {
  let missing = || refused(StatusCode::NOT_FOUND, "not-found", format!("There is no asset {name}."));
  if !ASSETS.contains(&name.as_str()) {
    return missing();
  }
  match tokio::fs::read(app.engine.data_dir().root().join(&name)).await {
    Ok(body) => {
      ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], body).into_response()
    }
    Err(_) => missing(),
  }
}
