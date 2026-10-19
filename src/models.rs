//! Model identifiers: their estimated rates, the backend that bills them, and the readable name every
//! screen shows.

use serde::{Deserialize, Serialize};

/// USD per million tokens, estimates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
  pub input: f64,
  pub cache_write: f64,
  pub cache_read: f64,
}

const fn rates(input: f64, cache_write: f64, cache_read: f64) -> Rates {
  Rates {
    input,
    cache_write,
    cache_read,
  }
}

/// Per model prefix, the first match wins.
pub const RATES: &[(&str, Rates)] = &[
  ("claude-fable-5-1", rates(10.0, 12.5, 0.25)),
  ("claude-fable-5", rates(10.0, 12.5, 0.25)),
  ("claude-opus-5", rates(5.0, 6.25, 0.5)),
  ("claude-sonnet-5", rates(3.0, 3.75, 0.3)),
  ("claude-haiku-4-5", rates(1.0, 1.25, 0.1)),
  ("claude-opus-4", rates(15.0, 18.75, 1.5)),
  ("claude-sonnet-4", rates(3.0, 3.75, 0.3)),
];

/// An Anthropic model the table does not list is priced as the dearest.
const UNLISTED_ANTHROPIC: Rates = rates(10.0, 12.5, 0.25);
const FREE: Rates = rates(0.0, 0.0, 0.0);

/// An event row's `backend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
  Anthropic,
  Local,
  Openai,
  Openrouter,
}

impl Backend {
  pub fn as_str(self) -> &'static str {
    match self {
      Backend::Anthropic => "anthropic",
      Backend::Local => "local",
      Backend::Openai => "openai",
      Backend::Openrouter => "openrouter",
    }
  }
}

/// A transcript model's rates and backend: a `claude` model bills Anthropic, anything else is a local
/// backend that bills nothing.
pub fn rate_for(model: &str) -> (Rates, Backend) {
  let model = model.to_lowercase();
  if !model.starts_with("claude") {
    return (FREE, Backend::Local);
  }
  let rates = RATES
    .iter()
    .find(|(prefix, _)| model.starts_with(prefix))
    .map_or(UNLISTED_ANTHROPIC, |&(_, r)| r);
  (rates, Backend::Anthropic)
}

/// Model identifier prefix → readable name, the first match wins. The table, the page and the alert
/// text all read this one list, so a model never shows under two names.
pub const MODEL_NAMES: &[(&str, &str)] = &[
  ("claude-fable-5-1", "Fable 5.1"),
  ("claude-fable-5", "Fable 5"),
  ("claude-opus-5", "Opus 5"),
  ("claude-opus-4-8", "Opus 4.8"),
  ("claude-opus-4", "Opus 4"),
  ("claude-sonnet-5", "Sonnet 5"),
  ("claude-sonnet-4", "Sonnet 4"),
  ("claude-haiku-4-5", "Haiku 4.5"),
  ("claude-haiku", "Haiku"),
];

/// The readable name of a model: its listed name, `sys` for the CLI's synthetic lines, `?` for none,
/// else the identifier itself.
pub fn model_name(model: &str) -> &str {
  if let Some((_, name)) = MODEL_NAMES.iter().find(|(prefix, _)| model.starts_with(prefix)) {
    return name;
  }
  match model {
    "<synthetic>" => "sys",
    "" => "?",
    _ => model,
  }
}
