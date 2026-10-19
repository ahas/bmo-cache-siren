//! One request of a runner that leaves no transcript, posted to `POST /events` (docs/api.md): its twelve
//! fields checked and turned into the common event row.

use chrono::{DateTime, Local};
use serde_json::{Map, Value};

use crate::attribution::Kind;
use crate::config::Config;
use crate::event::{Event, Level, first_chars, stamp};
use crate::models::{Backend, model_name};

/// The longest string field a caller may post.
const MAX_FIELD_CHARS: usize = 500;

/// Check a posted body and make its row. `seen` says whether the ring already holds a row of the same
/// mode, session and model. A body breaking a rule is refused with an English message naming the field.
pub fn posted_event(
  body: &Value,
  seen: impl FnOnce(&str, &str, &str) -> bool,
  config: &Config,
  at: &DateTime<Local>,
) -> Result<Event, String> {
  let Value::Object(body) = body else {
    return Err("body must be an object".into());
  };
  let mode = match body.get("mode").and_then(Value::as_str) {
    Some(mode @ ("codex" | "openrouter")) => mode,
    _ => return Err("mode must be codex or openrouter".into()),
  };
  let model = word(body, "model", true)?;
  let key = word(body, "key", true)?;
  let agent = word(body, "agent", false)?;
  let project = word(body, "project", false)?;
  let project = if project.is_empty() { key } else { project };
  let cwd = word(body, "cwd", false)?;
  let sid = word(body, "session", false)?;
  let input = count(body, "input_tokens")?;
  let cc = count(body, "cache_write_input_tokens")?;
  let read = count(body, "cache_read_input_tokens")?;
  let out = count(body, "output_tokens")?;
  let known = body.get("usage_known") == Some(&Value::Bool(true));
  let new = input + cc;
  let seen = !sid.is_empty() && seen(mode, &sid, &model);

  let past_yellow = new >= config.yellow_new_tokens;
  let red_or_yellow = if new >= config.red_new_tokens { Level::Red } else { Level::Yellow };
  let (kind, note, level) = if !known {
    (Kind::Request, "usage_unavailable", Level::Info)
  } else if read > 0 && past_yellow {
    (Kind::Growth, "", red_or_yellow)
  } else if read > 0 {
    (Kind::Warm, "", Level::Info)
  } else if seen && past_yellow {
    (Kind::Cold, "unknown", red_or_yellow)
  } else {
    let level = if past_yellow { Level::Yellow } else { Level::Info };
    if seen { (Kind::Request, "", level) } else { (Kind::Start, "session_start", level) }
  };

  let (ts_kst, date_kst, epoch) = stamp(at);
  Ok(Event {
    ts_kst,
    date_kst,
    epoch,
    project,
    session: first_chars(&sid, 8),
    sid,
    agent: Value::String(agent),
    parent: String::new(),
    cwd,
    mode: mode.into(),
    m: model_name(&model).into(),
    model,
    backend: if mode == "codex" { Backend::Openai } else { Backend::Openrouter },
    kind,
    level,
    note: note.into(),
    new,
    input,
    cc,
    read,
    out,
    ctx: new + read,
    prev_ctx: 0,
    gap_s: None,
    cost: 0.0,
    req: String::new(),
  })
}

/// A string field, missing read as empty, trimmed; a required one must not be blank.
fn word(body: &Map<String, Value>, name: &str, required: bool) -> Result<String, String> {
  let refused = || format!("{name} must be a short string");
  let value = match body.get(name) {
    None => "",
    Some(Value::String(value)) => value.as_str(),
    Some(_) => return Err(refused()),
  };
  if value.chars().count() > MAX_FIELD_CHARS || (required && value.trim().is_empty()) {
    return Err(refused());
  }
  Ok(value.trim().into())
}

/// A token count, missing read as 0; a fraction is cut to its whole part.
fn count(body: &Map<String, Value>, name: &str) -> Result<u64, String> {
  match body.get(name) {
    None => Ok(0),
    Some(Value::Number(number)) => match number.as_u64() {
      Some(whole) => Ok(whole),
      None => match number.as_f64() {
        Some(value) if value >= 0.0 => Ok(value.trunc() as u64),
        _ => Err(format!("{name} must be a non-negative number")),
      },
    },
    Some(_) => Err(format!("{name} must be a non-negative number")),
  }
}
