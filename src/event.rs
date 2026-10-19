//! An event row (docs/api.md, `GET /events`): the row the siren makes of one request, a limit, a notice,
//! a command or a compact, and the alert level it carries.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::attribution::Kind;
use crate::clock::epoch_of;
use crate::config::Config;
use crate::models::Backend;

/// An event row's `level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
  Info,
  Yellow,
  /// Sounds the siren.
  Red,
}

/// A transcript row's level: red for a cold or growth request at or past the red line and for a
/// refusal; yellow for a cold, growth or start request at or past the yellow line and for a limit, an
/// error or a compact; info otherwise. A local backend bills nothing, so it never sounds: its red is
/// yellow.
pub fn level_for(kind: Kind, new: u64, backend: Backend, config: &Config) -> Level {
  let level = match kind {
    Kind::Cold | Kind::Growth if new >= config.red_new_tokens => Level::Red,
    Kind::Refusal => Level::Red,
    Kind::Cold | Kind::Growth | Kind::Start if new >= config.yellow_new_tokens => Level::Yellow,
    Kind::Quota | Kind::Error | Kind::Compact => Level::Yellow,
    _ => Level::Info,
  };
  if backend == Backend::Local && level == Level::Red { Level::Yellow } else { level }
}

/// One request or transcript mark, before the sequence number the ring gives it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
  /// Host-local `HH:MM:SS`.
  pub ts_kst: String,
  /// Host-local `MM-DD`.
  pub date_kst: String,
  pub epoch: f64,
  pub project: String,
  pub session: String,
  pub sid: String,
  /// A transcript row: whether it is a subagent's; a posted row: the caller's agent name.
  pub agent: Value,
  pub parent: String,
  pub cwd: String,
  pub mode: String,
  pub model: String,
  pub m: String,
  pub backend: Backend,
  pub kind: Kind,
  pub level: Level,
  pub note: String,
  pub new: u64,
  pub input: u64,
  pub cc: u64,
  pub read: u64,
  pub out: u64,
  pub ctx: u64,
  pub prev_ctx: u64,
  pub gap_s: Option<i64>,
  pub cost: f64,
  pub req: String,
}

/// The three time fields of a row made at `at`.
pub fn stamp(at: &DateTime<Local>) -> (String, String, f64) {
  (at.format("%H:%M:%S").to_string(), at.format("%m-%d").to_string(), epoch_of(at))
}

impl Event {
  /// The row as the ring holds it and the routes answer it, `seq` last.
  pub fn into_row(self, seq: u64) -> Map<String, Value> {
    let Ok(Value::Object(mut row)) = serde_json::to_value(self) else {
      unreachable!("an event serialises to an object");
    };
    row.insert("seq".into(), seq.into());
    row
  }
}

/// The first `count` characters of `text`.
pub fn first_chars(text: &str, count: usize) -> String {
  text.chars().take(count).collect()
}

/// `text` with every `<...>` span of at least one character read as a space.
///
/// The CLI wraps its command lines and cross-session messages in tags of its own, and the request label
/// shows only the words between them; the CLI's own wrapper is read here, no general markup.
pub fn strip_tags(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut rest = text;
  while let Some(open) = rest.find('<') {
    out.push_str(&rest[..open]);
    let after = &rest[open + 1..];
    match after.find('>') {
      Some(close) if close > 0 => {
        out.push(' ');
        rest = &after[close + 1..];
      }
      _ => {
        out.push('<');
        rest = after;
      }
    }
  }
  out.push_str(rest);
  out
}

/// `text` with every run of whitespace read as one space and none at either end.
pub fn squeeze(text: &str) -> String {
  text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A count with a comma between every three digits.
pub fn thousands(count: u64) -> String {
  let digits = count.to_string();
  let mut out = String::with_capacity(digits.len() + digits.len() / 3);
  for (index, digit) in digits.chars().enumerate() {
    if index > 0 && (digits.len() - index).is_multiple_of(3) {
      out.push(',');
    }
    out.push(digit);
  }
  out
}
