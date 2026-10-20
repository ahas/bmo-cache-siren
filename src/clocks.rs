//! The daemon's own clocks: the transcript scan, the event-log prune and the alive line.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::engine::Engine;

/// Scan passes between two saves of the transcript offsets.
const SAVE_EVERY_PASSES: u64 = 30;
/// The first prune waits out the start-up rush.
const FIRST_PRUNE_AFTER: Duration = Duration::from_secs(30);
/// The prune never runs more often than this.
const PRUNE_AT_MOST_EVERY_S: f64 = 600.0;
/// One alive line this often, even with no requests, so a log-staleness watcher does not misfire.
const ALIVE_EVERY: Duration = Duration::from_secs(600);

/// Start the scan, prune and alive clocks, each on a thread of its own.
pub fn start(engine: Arc<Engine>) {
  let scanning = engine.clone();
  thread::spawn(move || scan_loop(&scanning));
  let pruning = engine.clone();
  thread::spawn(move || prune_loop(&pruning));
  thread::spawn(move || alive_loop(&engine));
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

/// Write an alive line every ten minutes.
fn alive_loop(engine: &Engine) {
  loop {
    let window = engine.window();
    engine.log().info(
      "heartbeat alive",
      json!({"events": window.len(), "files": engine.files_watched(),
             "last_event": window.last().map(|event| event.id)}),
    );
    thread::sleep(ALIVE_EVERY);
  }
}
