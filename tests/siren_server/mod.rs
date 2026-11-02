//! A real bmo-cache-siren HTTP server for one test: an ephemeral loopback port, a temporary data directory
//! and its own config file, read the way the daemon reads it at start. No transcript is scanned and no
//! background clock runs; only the routes do. The server stops when the value is dropped.
//!
//! Each test file uses the part it needs, so the rest reads as unused there.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use cache_siren::client::SirenClient;
use cache_siren::{App, Config, DataDir, Engine, Log, Silent, load_config, router};
use serde_json::Value;
use tokio::sync::oneshot;

/// The token every test server takes.
pub const TOKEN: &str = "test-token";

pub struct SirenServer {
  pub dir: tempfile::TempDir,
  pub config_path: PathBuf,
  pub app: Arc<App>,
  base: String,
  http: reqwest::Client,
  stop: Option<oneshot::Sender<()>>,
}

impl SirenServer {
  /// Start a server; `config`, when given, is written to the config file before it is read.
  pub async fn start(config: Option<Value>) -> SirenServer {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    if let Some(config) = config {
      std::fs::write(&config_path, config.to_string()).unwrap();
    }
    let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
    let config = load_config(&config_path, Config::defaults(dir.path()), &log);
    let engine = Arc::new(Engine::new(config, DataDir::new(dir.path()), log, Arc::new(Silent)));
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
    let app = Arc::new(App::new(engine, web, Some(config_path.clone()), TOKEN));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let routes = router(app.clone());
    let stopping = app.clone();
    tokio::spawn(async move {
      let _ = axum::serve(listener, routes)
        .with_graceful_shutdown(async move {
          let _ = stopped.await;
          stopping.stop();
        })
        .await;
    });
    SirenServer {
      dir,
      config_path,
      app,
      base,
      http: reqwest::Client::new(),
      stop: Some(stop),
    }
  }

  /// The address of one path on this server.
  pub fn url(&self, path: &str) -> String {
    format!("{}{path}", self.base)
  }

  pub fn engine(&self) -> &Engine {
    self.app.engine()
  }

  /// What makes the service's own client of this server with its token. The client waits on its
  /// answers, so it is made and used where blocking is allowed, such as a blocking task.
  pub fn client(&self) -> impl FnOnce() -> SirenClient + Send + 'static {
    let base = self.base.clone();
    move || SirenClient::new(base, TOKEN)
  }

  /// A request carrying the token and asking for JSON.
  pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    self
      .http
      .request(method, self.url(path))
      .bearer_auth(TOKEN)
      .header("Accept", "application/json")
  }

  /// One request; the status and the decoded JSON body, for success and error answers alike.
  pub async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> (u16, Value) {
    let mut request = self.request(method, path);
    if let Some(body) = body {
      request = request.json(body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
  }

  pub async fn get(&self, path: &str) -> (u16, Value) {
    self.call(reqwest::Method::GET, path, None).await
  }

  pub async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
    self.call(reqwest::Method::POST, path, Some(body)).await
  }

  pub async fn patch(&self, path: &str, body: &Value) -> (u16, Value) {
    self.call(reqwest::Method::PATCH, path, Some(body)).await
  }

  pub fn config_file(&self) -> Value {
    serde_json::from_slice(&std::fs::read(&self.config_path).unwrap()).unwrap()
  }
}

impl Drop for SirenServer {
  fn drop(&mut self) {
    if let Some(stop) = self.stop.take() {
      let _ = stop.send(());
    }
  }
}
