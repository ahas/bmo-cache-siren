//! One line of a Claude Code transcript read into an event (docs/api.md, Transcript roots): an assistant
//! request with its usage, a synthetic notice, limit or error, a refusal fallback, a model or effort
//! command, a compact; user lines only label the next request and mark images and tool results.

use serde_json::{Map, Value};

use crate::attribution::{Kind, Request};
use crate::clock::{epoch_of, now_local, ts_local};
use crate::config::Config;
use crate::event::{Event, first_chars, level_for, squeeze, stamp, strip_tags};
use crate::models::{model_name, rate_for};
use crate::session::Session;

/// Past this many remembered tool names, the oldest half is let go.
const TOOL_NAMES_KEPT: usize = 400;
const TOOL_NAMES_DROPPED: usize = 200;
/// A request label keeps this many characters.
const LABEL_CHARS: usize = 70;
/// A note keeps this many characters.
const NOTE_CHARS: usize = 90;

/// Which transcript a line came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
  /// The transcript's file name less `.jsonl`.
  pub sid: String,
  pub project: String,
  pub is_agent: bool,
  pub parent: Option<String>,
}

/// Read one parsed transcript line of `session`, which it updates; the event it makes, if any.
pub fn read_line(line: &Value, origin: &Origin, session: &mut Session, config: &Config) -> Option<Event> {
  let empty = Map::new();
  let line_map = line.as_object().unwrap_or(&empty);
  match line_map.get("type").and_then(Value::as_str) {
    Some("user") => {
      read_user(line_map, session);
      None
    }
    Some("system") => read_system(line_map, origin, session, config),
    Some("summary") => {
      session.flags.compact = true;
      None
    }
    Some("assistant") => read_assistant(line_map, origin, session, config),
    _ => None,
  }
}

/// A user line labels the request it prompts: a command, a cross-session message, an interrupt, typed
/// text or the tools whose results it carries; an image in it is marked.
fn read_user(line: &Map<String, Value>, session: &mut Session) {
  let meta = truthy(line.get("isMeta"));
  let message = object(line.get("message"));
  match message.and_then(|message| message.get("content")) {
    Some(Value::String(content)) => {
      let text = content.trim();
      session.last_req = if text.starts_with("<local-command") || text.starts_with("<command-name>") {
        if text.contains("/model") || text.contains("/effort") {
          session.flags.effort_model = true;
        }
        format!("⌘ {}", first_chars(strip_tags(text).trim(), LABEL_CHARS))
      } else if meta && text.contains("cross-session") {
        format!("✉ {}", first_chars(&squeeze(&strip_tags(text)), LABEL_CHARS))
      } else if text.starts_with("[Request interrupted") {
        "⏹ interrupted".into()
      } else {
        format!("⌨ {}", first_chars(&squeeze(&strip_tags(text)), LABEL_CHARS))
      };
    }
    Some(Value::Array(parts)) => {
      let mut names = Vec::new();
      let mut image = false;
      let mut text = "";
      for part in parts.iter().filter_map(Value::as_object) {
        match part.get("type").and_then(Value::as_str) {
          Some("tool_result") => {
            let id = part.get("tool_use_id").and_then(Value::as_str);
            names.push(
              id.and_then(|id| session.tool_names.get(id)).map_or("tool", String::as_str).to_string(),
            );
          }
          Some("image") => image = true,
          Some("text") if text.is_empty() => text = part.get("text").and_then(Value::as_str).unwrap_or(""),
          _ => {}
        }
      }
      if image {
        session.flags.image = true;
      }
      if !names.is_empty() {
        let more = if names.len() > 3 { format!(" +{}", names.len() - 3) } else { String::new() };
        session.last_req = format!("⚙ {}{more}", names[..names.len().min(3)].join(","));
      } else if !text.is_empty() {
        let mark = if meta { "✉" } else { "⌨" };
        session.last_req = format!("{mark} {}", first_chars(&squeeze(&strip_tags(text)), LABEL_CHARS));
      }
    }
    _ => {}
  }
}

/// A system line: a safeguard refusal fallback, a `/model` or `/effort` command, a compact boundary.
fn read_system(
  line: &Map<String, Value>,
  origin: &Origin,
  session: &mut Session,
  config: &Config,
) -> Option<Event> {
  let mark = Mark {
    line,
    origin,
    config,
  };
  match line.get("subtype").and_then(Value::as_str) {
    Some("model_refusal_fallback") => {
      session.flags.refusal = true;
      let fallback = line.get("fallbackModel").and_then(Value::as_str).unwrap_or("");
      let note = format!(
        "{} → {} (scope {})",
        text_of(line.get("apiRefusalCategory")),
        model_name(fallback),
        text_of(line.get("scope"))
      );
      let model = line.get("originalModel").and_then(Value::as_str).unwrap_or("");
      Some(mark.event(session, Kind::Refusal, model, note))
    }
    Some("local_command") => {
      let content = match line.get("content") {
        None => String::new(),
        Some(Value::String(content)) => content.clone(),
        Some(other) => other.to_string(),
      };
      if !(content.contains("/model") || content.contains("/effort")) {
        return None;
      }
      session.flags.effort_model = true;
      if !(content.contains("Set model") || content.to_lowercase().contains("effort")) {
        return None;
      }
      let model = session.last_model.clone().unwrap_or_default();
      let note = first_chars(strip_tags(&content).trim(), NOTE_CHARS);
      Some(mark.event(session, Kind::Cmd, &model, note))
    }
    Some("compact_boundary") => {
      session.flags.compact = true;
      let model = session.last_model.clone().unwrap_or_default();
      Some(mark.event(session, Kind::Compact, &model, "compact".into()))
    }
    _ => None,
  }
}

/// An assistant line: its tool calls are remembered; a synthetic or API-error line is a notice, a limit
/// or an error; a line with usage, once per message id, is a request.
fn read_assistant(
  line: &Map<String, Value>,
  origin: &Origin,
  session: &mut Session,
  config: &Config,
) -> Option<Event> {
  let empty = Map::new();
  let message = object(line.get("message")).unwrap_or(&empty);
  let model = message.get("model").and_then(Value::as_str).unwrap_or("");
  let content = message.get("content");
  if let Some(Value::Array(parts)) = content {
    remember_tools(parts, session);
  }

  let api_error = truthy(line.get("isApiErrorMessage"));
  let mark = Mark {
    line,
    origin,
    config,
  };
  if api_error || model == "<synthetic>" {
    let text = match content {
      Some(Value::Array(parts)) => parts
        .iter()
        .filter_map(Value::as_object)
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .map(|part| part.get("text").and_then(Value::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" "),
      Some(Value::String(text)) => text.clone(),
      _ => String::new(),
    };
    // Only the API-error mark the CLI adds says failure. A synthetic line without it is a notice the
    // CLI leaves from a turn with no answer to give, a compact say, so it raises no alert. A limit is a
    // wall, not a failure: the CLI words its session, weekly and monthly-spend limits in the one shape
    // "hit your ... limit", so the shape is matched; an older CLI's usage limit stands in its place.
    let kind = if api_error {
      let low = text.to_lowercase();
      let limit = low.contains("session limit")
        || low.contains("usage limit")
        || (low.contains("hit your") && low.contains("limit"));
      if limit { Kind::Quota } else { Kind::Error }
    } else {
      Kind::Notice
    };
    let key = format!("err\u{0}{}\u{0}{}", text_of(line.get("timestamp")), first_chars(&text, 40));
    if !session.remember(&key) {
      return None;
    }
    let model = session
      .last_model
      .clone()
      .filter(|last| !last.is_empty())
      .unwrap_or_else(|| model.into());
    return Some(mark.event(session, kind, &model, first_chars(&text, NOTE_CHARS)));
  }

  let usage = object(message.get("usage")).filter(|usage| !usage.is_empty())?;
  let msg_id = message.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
  if !session.remember(msg_id) {
    return None;
  }
  let input = tokens(usage.get("input_tokens"));
  let cc = tokens(usage.get("cache_creation_input_tokens"));
  let read = tokens(usage.get("cache_read_input_tokens"));
  let out = tokens(usage.get("output_tokens"));
  let new = input + cc;
  let at = line.get("timestamp").and_then(Value::as_str).and_then(ts_local).map(|at| epoch_of(&at));
  let gap_s = match (at, session.last_ts) {
    (Some(at), Some(last)) => Some(at - last),
    _ => None,
  };
  let kind = if !session.seen {
    Kind::Start
  } else if read == 0 && new >= config.yellow_new_tokens {
    Kind::Cold
  } else if new >= config.yellow_new_tokens {
    Kind::Growth
  } else {
    Kind::Warm
  };
  let cause = config.attribute(&Request {
    kind,
    flags: session.flags,
    mode: "claude-code",
    model,
    last_model: session.last_model.as_deref(),
    gap_s,
    read,
    prev_ctx: session.last_ctx,
  });
  let note = cause.map_or("", |cause| cause.as_str()).to_string();
  let mut event = mark.event(session, kind, model, note);
  let (rates, _) = rate_for(model);
  let cost =
    (input as f64 * rates.input + cc as f64 * rates.cache_write + read as f64 * rates.cache_read) / 1e6;
  event.level = level_for(kind, new, event.backend, config);
  event.new = new;
  event.input = input;
  event.cc = cc;
  event.read = read;
  event.out = out;
  event.ctx = new + read;
  event.prev_ctx = session.last_ctx;
  event.gap_s = gap_s.map(|gap| gap.trunc() as i64);
  event.cost = (cost * 1000.0).round() / 1000.0;

  session.seen = true;
  session.last_ts = at.or(session.last_ts);
  session.last_model = Some(model.into());
  session.last_ctx = new + read;
  session.flags = Default::default();
  Some(event)
}

/// Remember the names of the tools an assistant line calls, and mark a tool search.
fn remember_tools(parts: &[Value], session: &mut Session) {
  for part in parts.iter().filter_map(Value::as_object) {
    if part.get("type").and_then(Value::as_str) != Some("tool_use") {
      continue;
    }
    let name = part.get("name").and_then(Value::as_str);
    if let Some(id) = part.get("id").and_then(Value::as_str) {
      session.tool_names.insert(id.into(), name.unwrap_or("tool").into());
    }
    if name == Some("ToolSearch") {
      session.flags.tool_search = true;
    }
  }
  if session.tool_names.len() > TOOL_NAMES_KEPT {
    session.tool_names.drain(..TOOL_NAMES_DROPPED);
  }
}

/// What every row made from one transcript line shares.
struct Mark<'a> {
  line: &'a Map<String, Value>,
  origin: &'a Origin,
  config: &'a Config,
}

impl Mark<'_> {
  /// A row of `kind` with no token counts, timed by the line's timestamp, else now.
  fn event(&self, session: &Session, kind: Kind, model: &str, note: String) -> Event {
    let at = self
      .line
      .get("timestamp")
      .and_then(Value::as_str)
      .and_then(ts_local)
      .unwrap_or_else(now_local);
    let (ts_kst, date_kst, epoch) = stamp(&at);
    let (_, backend) = rate_for(model);
    let cwd = self.line.get("cwd").and_then(Value::as_str).unwrap_or("");
    Event {
      ts_kst,
      date_kst,
      epoch,
      project: self.origin.project.clone(),
      session: first_chars(&self.origin.sid, 8),
      sid: self.origin.sid.clone(),
      agent: Value::Bool(self.origin.is_agent),
      parent: first_chars(self.origin.parent.as_deref().unwrap_or(""), 8),
      cwd: cwd.rsplit('/').next().unwrap_or("").into(),
      mode: "claude-code".into(),
      model: model.into(),
      m: model_name(model).into(),
      backend,
      kind,
      level: level_for(kind, 0, backend, self.config),
      note,
      new: 0,
      input: 0,
      cc: 0,
      read: 0,
      out: 0,
      ctx: 0,
      prev_ctx: 0,
      gap_s: None,
      cost: 0.0,
      req: session.last_req.clone(),
    }
  }
}

fn object(value: Option<&Value>) -> Option<&Map<String, Value>> {
  value.and_then(Value::as_object)
}

/// A token count as the transcript gives it; a fraction is cut, anything else is 0.
fn tokens(value: Option<&Value>) -> u64 {
  match value {
    Some(Value::Number(number)) => {
      number.as_u64().unwrap_or_else(|| number.as_f64().map_or(0, |n| n.max(0.0) as u64))
    }
    _ => 0,
  }
}

/// Whether a value counts as set: present, and neither null, false, zero nor empty.
fn truthy(value: Option<&Value>) -> bool {
  match value {
    None | Some(Value::Null) => false,
    Some(Value::Bool(flag)) => *flag,
    Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
    Some(Value::String(text)) => !text.is_empty(),
    Some(Value::Array(items)) => !items.is_empty(),
    Some(Value::Object(map)) => !map.is_empty(),
  }
}

/// A field as it reads in a note: its text, `None` where it is missing.
fn text_of(value: Option<&Value>) -> String {
  match value {
    None | Some(Value::Null) => "None".into(),
    Some(Value::String(text)) => text.clone(),
    Some(other) => other.to_string(),
  }
}
