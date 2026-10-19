//! A real cache-siren HTTP server for one test: an ephemeral loopback port, a temporary data directory
//! and its own config file, read the way the daemon reads it at start. No transcript is scanned and no
//! background clock runs; only the routes do. The server stops when the value is dropped.
//!
//! Each test file uses the part it needs, so the rest reads as unused there.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use cache_siren::{App, Config, DataDir, Engine, Log, Silent, load_config, router};
use serde_json::Value;
use tokio::sync::oneshot;

pub struct SirenServer {
  pub dir: tempfile::TempDir,
  pub config_path: PathBuf,
  pub app: Arc<App>,
  base: String,
  client: reqwest::Client,
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
    let config = load_config(&config_path, SirenServer::defaults(&dir), &log);
    let engine = Arc::new(Engine::new(config, DataDir::new(dir.path()), log, Arc::new(Silent)));
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
    let app = Arc::new(App::new(engine, web, Some(config_path.clone())));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let routes = router(app.clone());
    tokio::spawn(async move {
      let _ = axum::serve(listener, routes)
        .with_graceful_shutdown(async {
          let _ = stopped.await;
        })
        .await;
    });
    SirenServer {
      dir,
      config_path,
      app,
      base,
      client: reqwest::Client::new(),
      stop: Some(stop),
    }
  }

  /// The defaults of a daemon whose data directory is this server's.
  pub fn defaults(dir: &tempfile::TempDir) -> Config {
    Config::defaults(dir.path())
  }

  /// The address of one path on this server.
  pub fn url(&self, path: &str) -> String {
    format!("{}{path}", self.base)
  }

  pub fn engine(&self) -> &Engine {
    self.app.engine()
  }

  /// One request; the status and the decoded JSON body, for success and error answers alike.
  pub async fn call(&self, method: reqwest::Method, path: &str, body: Option<Vec<u8>>) -> (u16, Value) {
    let mut request = self.client.request(method, format!("{}{path}", self.base));
    if let Some(body) = body {
      request = request.header("Content-Type", "application/json").body(body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
  }

  pub async fn get(&self, path: &str) -> (u16, Value) {
    self.call(reqwest::Method::GET, path, None).await
  }

  pub async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
    self.call(reqwest::Method::POST, path, Some(body.to_string().into_bytes())).await
  }

  pub async fn post_raw(&self, path: &str, raw: &[u8]) -> (u16, Value) {
    self.call(reqwest::Method::POST, path, Some(raw.to_vec())).await
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
