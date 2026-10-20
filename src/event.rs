//! The alert level a Usage carries (docs/api.md, Usage), and the text helpers its label and note are
//! written with.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::attribution::Kind;
use crate::config::Config;
use crate::models::Backend;

/// A Usage's `level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
  Info,
  Yellow,
  /// Sounds the siren.
  Red,
}

/// A transcript Usage's level: red for a cold or growth request at or past the red line and for a
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
