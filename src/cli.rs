//! The command-line verbs against a running daemon, and the lines they print.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use serde_json::{Value, json};

use crate::event::thousands;

/// What a verb prints when the daemon gives no readable answer.
pub const NOT_RESPONDING: &str = "❌ cache-siren daemon not responding";
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// The test siren plays and speaks before it answers, so it is given longer.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A field as it reads in a line: its text, `None` where it is missing.
fn text(value: Option<&Value>) -> String {
  match value {
    None | Some(Value::Null) => "None".into(),
    Some(Value::String(text)) => text.clone(),
    Some(Value::Bool(true)) => "True".into(),
    Some(Value::Bool(false)) => "False".into(),
    Some(other) => other.to_string(),
  }
}

/// A string field, empty where it is missing.
fn word(row: &Value, key: &str) -> String {
  match row.get(key) {
    None => String::new(),
    other => text(other),
  }
}

fn count(row: &Value, key: &str) -> u64 {
  row
    .get(key)
    .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
    .unwrap_or(0)
}

fn first(text: &str, chars: usize) -> String {
  text.chars().take(chars).collect()
}

/// The `status` line, from `GET /health`.
pub fn status_line(health: &Value) -> String {
  let five_h = health.get("quota").and_then(|quota| quota.get("five_h"));
  format!(
    "✅ cache-siren up {}s · files {} · events {} · last {} · 5h {}%",
    text(health.get("uptime_s")),
    text(health.get("files")),
    text(health.get("events")),
    text(health.get("last_event")),
    text(five_h)
  )
}

/// The `tail` lines, one per row of `GET /events`.
pub fn tail_lines(feed: &Value) -> Vec<String> {
  let rows = feed.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
  rows
    .iter()
    .map(|row| {
      format!(
        "{:8} {:12} {:8} {:9} {:7} +{:>8} read={:>9} {:14} {}",
        word(row, "ts_kst"),
        first(&word(row, "project"), 12),
        word(row, "session"),
        word(row, "m"),
        word(row, "kind"),
        thousands(count(row, "new")),
        thousands(count(row, "read")),
        first(&word(row, "note"), 14),
        first(&word(row, "req"), 60)
      )
    })
    .collect()
}

/// What `toggle` sets `visible` to, from `GET /ctl`: the opposite of what it is, visible where unsaid.
pub fn toggled(ctl: &Value) -> bool {
  !ctl.get("visible").and_then(Value::as_bool).unwrap_or(true)
}

/// The verbs, each against the daemon on `127.0.0.1:<port>`.
pub struct Cli {
  base: String,
  client: reqwest::blocking::Client,
}

impl Cli {
  pub fn new(port: u16) -> Cli {
    Cli {
      base: format!("http://127.0.0.1:{port}"),
      client: reqwest::blocking::Client::new(),
    }
  }

  fn get(&self, path: &str) -> Option<Value> {
    let answer = self.client.get(format!("{}{path}", self.base)).timeout(CALL_TIMEOUT).send().ok()?;
    answer.json().ok()
  }

  fn post(&self, path: &str, body: &Value, timeout: Duration) -> Option<String> {
    let request = self.client.post(format!("{}{path}", self.base)).timeout(timeout).body(body.to_string());
    request.send().ok()?.text().ok()
  }

  /// Set overlay keys; `said` is printed once the daemon has answered.
  fn set(&self, body: Value, said: &str) -> ExitCode {
    match self.post("/ctl", &body, CALL_TIMEOUT) {
      Some(_) => {
        println!("{said}");
        ExitCode::SUCCESS
      }
      None => ExitCode::FAILURE,
    }
  }

  pub fn status(&self) -> ExitCode {
    match self.get("/health") {
      Some(health) => {
        println!("{}", status_line(&health));
        ExitCode::SUCCESS
      }
      None => not_responding(),
    }
  }

  pub fn show(&self, visible: bool) -> ExitCode {
    self.set(json!({"visible": visible}), if visible { "shown" } else { "hidden" })
  }

  pub fn toggle(&self) -> ExitCode {
    let Some(ctl) = self.get("/ctl") else { return not_responding() };
    let visible = toggled(&ctl);
    self.set(json!({"visible": visible}), &format!("visible={visible}"))
  }

  pub fn mute(&self, muted: bool) -> ExitCode {
    self.set(json!({"muted": muted}), if muted { "muted" } else { "unmuted" })
  }

  pub fn local(&self, on: bool) -> ExitCode {
    self.set(json!({"show_local": on}), &format!("show_local={on}"))
  }

  pub fn demo(&self) -> ExitCode {
    self.set(
      json!({"page": "/demo"}),
      "switched to demo (frozen screen); back to live: cache-siren live",
    )
  }

  pub fn live(&self) -> ExitCode {
    self.set(json!({"page": "/"}), "switched to live")
  }

  /// Sound the test siren and print the daemon's answer as it gave it.
  pub fn test(&self) -> ExitCode {
    let answer = self.post("/test-siren", &json!({}), TEST_TIMEOUT);
    println!("{}", answer.as_deref().unwrap_or(""));
    if answer.is_some() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
  }

  pub fn tail(&self, rows: &str) -> ExitCode {
    match self.get(&format!("/events?limit={rows}&local=1")) {
      Some(feed) => {
        for line in tail_lines(&feed) {
          println!("{line}");
        }
        ExitCode::SUCCESS
      }
      None => not_responding(),
    }
  }
}

fn not_responding() -> ExitCode {
  println!("{NOT_RESPONDING}");
  ExitCode::FAILURE
}

/// Restart the daemon under the host's service manager: the daemon and the overlay app under launchd
/// on macOS, the user unit under systemd elsewhere.
pub fn restart() -> ExitCode {
  let started = if cfg!(target_os = "macos") {
    let uid = Command::new("id")
      .arg("-u")
      .output()
      .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let Ok(uid) = uid else { return ExitCode::FAILURE };
    ["local.cache-siren", "local.cache-siren.app"].iter().all(|label| {
      Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/{label}")])
        .status()
        .is_ok_and(|status| status.success())
    })
  } else {
    Command::new("systemctl")
      .args(["--user", "restart", "cache-siren"])
      .status()
      .is_ok_and(|status| status.success())
  };
  if started {
    println!("restarted");
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

/// Print the last `lines` lines of the daemon's log.
pub fn log_tail(path: &Path, lines: usize) -> ExitCode {
  let Ok(text) = std::fs::read_to_string(path) else {
    eprintln!("cache-siren: cannot read {}", path.display());
    return ExitCode::FAILURE;
  };
  let all: Vec<&str> = text.lines().collect();
  for line in &all[all.len().saturating_sub(lines)..] {
    println!("{line}");
  }
  ExitCode::SUCCESS
}
