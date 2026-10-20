//! One request of a runner whose harness leaves no transcript, posted to `POST /events` (docs/api.md):
//! its fields checked and turned into a Usage classified as a transcript request is.

use crate::attribution::Kind;
use crate::config::Config;
use crate::event::Level;
use crate::models::{Backend, model_name, rate_for};
use crate::wire::{NewUsage, Usage};

/// The longest string field a caller may post.
const MAX_FIELD_CHARS: usize = 500;

/// The session's previous Usage in the window of the same harness and model, as `posted_usage` reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Previous {
  /// The window holds a Usage of the same harness, session and model.
  pub seen: bool,
  /// When the session's latest Usage went out, of any harness or model.
  pub at: Option<i64>,
  /// That Usage's context.
  pub context_token_count: u64,
}

/// Check a posted body and make its Usage. `previous` says what the window holds of its session. A body
/// breaking a rule is refused with an English sentence naming the field.
pub fn posted_usage(body: NewUsage, previous: Previous, config: &Config) -> Result<Usage, String> {
  let session_id = word("sessionId", Some(body.session_id), true)?;
  let model = word("modelName", Some(body.model_name), true)?;
  let harness = word("harnessName", body.harness_name, false)?;
  let scope = optional("scope", body.scope)?;
  let project = word("projectName", body.project_name, false)?;
  let project = if project.is_empty() { scope.clone().unwrap_or_default() } else { project };
  let directory = word("directoryName", body.directory_name, false)?;
  let actor = optional("actorName", body.actor_name)?;
  if body.at < 0 {
    return Err("at must be epoch milliseconds, at least 0".into());
  }
  let new = body.new_token_count;
  let read = body.cached_token_count;
  let cc = body.cache_write_token_count.unwrap_or(0);
  if cc > new {
    return Err("cacheWriteTokenCount must be at most newTokenCount".into());
  }
  let input = new - cc;
  let out = body.output_token_count.unwrap_or(0);

  let past_yellow = new >= config.yellow_new_tokens;
  let red_or_yellow = if new >= config.red_new_tokens { Level::Red } else { Level::Yellow };
  let (kind, note, level) = if read > 0 && past_yellow {
    (Kind::Growth, "", red_or_yellow)
  } else if read > 0 {
    (Kind::Warm, "", Level::Info)
  } else if previous.seen && past_yellow {
    (Kind::Cold, "unknown", red_or_yellow)
  } else {
    let level = if past_yellow { Level::Yellow } else { Level::Info };
    if previous.seen {
      (Kind::Request, "", level)
    } else {
      (Kind::Start, "session_start", level)
    }
  };

  let (rates, by_model) = rate_for(&model);
  let backend = match harness.as_str() {
    "codex" => Backend::Openai,
    "openrouter" => Backend::Openrouter,
    _ => by_model,
  };
  // A local backend bills nothing, so it never sounds.
  let level = if backend == Backend::Local && level == Level::Red { Level::Yellow } else { level };
  let cost = if backend == Backend::Anthropic {
    (input as f64 * rates.input + cc as f64 * rates.cache_write + read as f64 * rates.cache_read) / 1e6
  } else {
    0.0
  };
  Ok(Usage {
    model_label: model_name(&model).into(),
    model_name: model,
    at: body.at,
    harness_name: harness,
    backend,
    kind,
    level,
    note: note.into(),
    new_token_count: new,
    input_token_count: input,
    cache_write_token_count: cc,
    cached_token_count: read,
    output_token_count: out,
    context_token_count: new + read,
    previous_context_token_count: previous.context_token_count,
    gap_ms: previous.at.map(|at| body.at - at),
    cost_usd: (cost * 1000.0).round() / 1000.0,
    project_name: project,
    directory_name: directory,
    subagent: false,
    parent_session_id: None,
    request_label: String::new(),
    scope,
    actor_name: actor,
    labels: body.labels.unwrap_or_default(),
    session_id,
  })
}

/// A string field, missing read as empty, trimmed; a required one must not be blank.
fn word(name: &str, value: Option<String>, required: bool) -> Result<String, String> {
  let value = value.unwrap_or_default();
  if value.chars().count() > MAX_FIELD_CHARS {
    return Err(format!("{name} must be at most {MAX_FIELD_CHARS} characters"));
  }
  if required && value.trim().is_empty() {
    return Err(format!("{name} must not be blank"));
  }
  Ok(value.trim().into())
}

/// A string field the caller may leave out; blank reads as left out.
fn optional(name: &str, value: Option<String>) -> Result<Option<String>, String> {
  let value = word(name, value, false)?;
  Ok((!value.is_empty()).then_some(value))
}
