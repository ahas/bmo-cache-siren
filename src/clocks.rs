//! The daemon's own clocks: the transcript scan, the event-log prune, the quota poll and the heartbeat
//! (docs/api.md, Consumed contracts).

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::clock::now_local;
use crate::engine::Engine;
use crate::quota::quota_from;

/// Scan passes between two saves of the transcript offsets.
const SAVE_EVERY_PASSES: u64 = 30;
/// The first prune waits out the start-up rush.
const FIRST_PRUNE_AFTER: Duration = Duration::from_secs(30);
/// The prune never runs more often than this.
const PRUNE_AT_MOST_EVERY_S: f64 = 600.0;
const QUOTA_EVERY: Duration = Duration::from_secs(30);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(45);
/// One alive line this often, even with no requests, so a log-staleness watcher does not misfire.
const ALIVE_EVERY: Duration = Duration::from_secs(600);
const OUTSIDE_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of an outside failure a log line keeps.
const ERR_CHARS: usize = 120;

/// Start the scan and prune clocks on threads of their own, and the quota and heartbeat clocks on the
/// runtime this is called on.
pub fn start(engine: Arc<Engine>) {
  let scanning = engine.clone();
  thread::spawn(move || scan_loop(&scanning));
  let pruning = engine.clone();
  thread::spawn(move || prune_loop(&pruning));
  let client = reqwest::Client::builder().timeout(OUTSIDE_TIMEOUT).build().unwrap_or_default();
  tokio::spawn(quota_loop(engine.clone(), client.clone()));
  tokio::spawn(heartbeat_loop(engine, client));
}

/// Tail every recent transcript each `poll_s`, keeping the offsets every 30 passes.
fn scan_loop(engine: &Engine) {
  let pause = Duration::from_secs_f64(engine.config().poll_s.max(0.05));
  let mut passes = 0u64;
  loop {
    engine.scan_all();
    passes += 1;
    if passes.is_multiple_of(SAVE_EVERY_PASSES) {
      engine.save();
    }
    thread::sleep(pause);
  }
}

/// Prune the event log 30 seconds after start, then every `prune_every_s`, at least ten minutes apart.
fn prune_loop(engine: &Engine) {
  thread::sleep(FIRST_PRUNE_AFTER);
  loop {
    engine.prune_events();
    thread::sleep(Duration::from_secs_f64(engine.config().prune_every_s.max(PRUNE_AT_MOST_EVERY_S)));
  }
}

/// Read the quota source every 30 seconds; a failure is logged and the last reading kept. Where there is
/// no source nothing is asked, so the log does not fill with a failure every 30 seconds.
async fn quota_loop(engine: Arc<Engine>, client: reqwest::Client) {
  loop {
    let url = engine.config().quota_url;
    if !url.is_empty() {
      match fetch(&client, &url).await {
        Ok(answer) => engine.set_quota(quota_from(&answer, &now_local().format("%H:%M").to_string())),
        Err(why) => engine.log().warn("quota fetch failed", json!({"err": cut(&why)})),
      }
    }
    tokio::time::sleep(QUOTA_EVERY).await;
  }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Value, String> {
  let answer = client.get(url).send().await.map_err(|why| why.to_string())?;
  let answer = answer.error_for_status().map_err(|why| why.to_string())?;
  answer.json::<Value>().await.map_err(|why| why.to_string())
}

/// Write an alive line every ten minutes, and post the heartbeat every 45 seconds where the deployment
/// names a service registry.
async fn heartbeat_loop(engine: Arc<Engine>, client: reqwest::Client) {
  let mut alive_at: Option<Instant> = None;
  loop {
    if alive_at.is_none_or(|at| at.elapsed() >= ALIVE_EVERY) {
      alive_at = Some(Instant::now());
      let stats = engine.stats();
      engine.log().info(
        "heartbeat alive",
        json!({"events": engine.rows().len(), "files": stats["files"], "last_event": stats["last_event"],
               "uptime_s": stats["uptime_s"]}),
      );
    }
    let config = engine.config();
    if !config.heartbeat_url.is_empty() {
      let node = if config.node.is_empty() {
        gethostname::gethostname().to_string_lossy().into_owned()
      } else {
        config.node.clone()
      };
      let sent = client
        .post(&config.heartbeat_url)
        .json(&heartbeat_body(&node, config.port))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
      if let Err(why) = sent {
        engine.log().warn("heartbeat failed", json!({"err": cut(&why.to_string())}));
      }
    }
    tokio::time::sleep(HEARTBEAT_EVERY).await;
  }
}

/// The heartbeat's body (docs/api.md, Heartbeat).
pub fn heartbeat_body(node: &str, port: u16) -> Value {
  json!({
    "name": "cache-siren", "node": node, "port": port, "kind": "service", "engine": "monitor",
    "health_url": format!("http://127.0.0.1:{port}/health"), "endpoint": format!("http://127.0.0.1:{port}/"),
    "status": "up",
    "capabilities": {"notes": "live per-request cache/new-token watch + siren for Anthropic requests (transcript tail)"},
  })
}

fn cut(text: &str) -> String {
  text.chars().take(ERR_CHARS).collect()
}
