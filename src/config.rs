//! The configuration (docs/configuration.md): one JSON file read once at start over the defaults, and
//! the three keys behind the settings that `PATCH /settings` changes live and merges back into that file
//! (docs/api.md, Settings).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Number, Value, json};

use crate::attribution::{Cause, Request, attribute};
use crate::log::Log;
use crate::paths::default_data_dir;
use crate::wire::{Settings, SettingsPatch};

/// Per harness, a model prefix or `*` and the cache lifetime in seconds.
pub type TtlTable = IndexMap<String, IndexMap<String, Number>>;

/// Every key the daemon reads, in the order the created file lists them, and any key the file adds,
/// carried but unused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
  pub port: u16,
  pub poll_s: f64,
  pub projects_globs: Vec<String>,
  pub project_prefixes: Vec<String>,
  #[serde(serialize_with = "whole")]
  pub recent_hours: f64,
  #[serde(serialize_with = "whole")]
  pub backfill_hours: f64,
  pub yellow_new_tokens: u64,
  pub red_new_tokens: u64,
  #[serde(serialize_with = "whole")]
  pub burst_window_s: f64,
  pub burst_red_tokens: u64,
  #[serde(serialize_with = "whole")]
  pub siren_cooldown_s: f64,
  pub armed: bool,
  pub ttl_s: Number,
  pub ttl_by_mode_model_s: TtlTable,
  pub prefix_reuse_min: f64,
  pub voice: bool,
  pub voice_cmd: Vec<String>,
  pub sound: bool,
  pub siren_file: String,
  pub siren_player: Vec<String>,
  pub ring: usize,
  #[serde(serialize_with = "whole")]
  pub retention_days: f64,
  #[serde(serialize_with = "whole")]
  pub prune_every_s: f64,
  #[serde(flatten)]
  pub extra: Map<String, Value>,
}

/// A whole number of hours or seconds is written as an integer, as the defaults are given.
fn whole<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
  if value.fract() == 0.0 && value.abs() < 9.0e15 {
    serializer.serialize_i64(*value as i64)
  } else {
    serializer.serialize_f64(*value)
  }
}

/// The port the daemon listens on where its config file names none.
pub const DEFAULT_PORT: u16 = 9022;

/// The port the daemon reads from the config file at `path`, without writing it: the file's `port` over the
/// defaults, the default port where the file does not read as a config, and none where no file stands.
pub fn port_in(path: &Path) -> Option<u16> {
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(why) if why.kind() == io::ErrorKind::NotFound => return None,
    Err(_) => return Some(DEFAULT_PORT),
  };
  let defaults = Config::defaults(Path::new("."));
  Some(over_defaults(&defaults, &text).map_or(DEFAULT_PORT, |config| config.port))
}

impl Config {
  /// The defaults for a daemon whose data directory is `data_dir`, where the siren file is looked for.
  pub fn defaults(data_dir: &Path) -> Config {
    let mut ttl_by_mode_model_s = TtlTable::new();
    ttl_by_mode_model_s.insert("claude-code".into(), IndexMap::from([("*".into(), Number::from(3600))]));
    ttl_by_mode_model_s.insert("codex".into(), IndexMap::from([("gpt-5.6".into(), Number::from(1800))]));
    ttl_by_mode_model_s.insert("openrouter".into(), IndexMap::new());
    Config {
      port: DEFAULT_PORT,
      poll_s: 1.0,
      projects_globs: vec!["~/.claude/projects".into()],
      project_prefixes: Vec::new(),
      recent_hours: 6.0,
      backfill_hours: 3.0,
      yellow_new_tokens: 20_000,
      red_new_tokens: 100_000,
      burst_window_s: 60.0,
      burst_red_tokens: 500_000,
      siren_cooldown_s: 20.0,
      armed: true,
      ttl_s: Number::from(3600),
      ttl_by_mode_model_s,
      prefix_reuse_min: 0.5,
      voice: false,
      voice_cmd: Vec::new(),
      sound: true,
      siren_file: data_dir.join("warning.mp3").to_string_lossy().into_owned(),
      siren_player: Vec::new(),
      ring: 600,
      retention_days: 7.0,
      prune_every_s: 21_600.0,
      extra: Map::new(),
    }
  }

  /// The cache lifetime in seconds of one request: within its harness's table the longest model prefix,
  /// else that harness's `*`; none for a listed harness with neither, since its provider has no uniform
  /// cache contract; `ttl_s` for a harness the table does not list.
  pub fn ttl_for(&self, mode: &str, model: &str) -> Option<f64> {
    let Some(rows) = self.ttl_by_mode_model_s.get(mode) else {
      return self.ttl_s.as_f64();
    };
    let mut named: Option<(&str, &Number)> = None;
    for (prefix, seconds) in rows {
      if prefix != "*"
        && model.starts_with(prefix.as_str())
        && named.is_none_or(|(best, _)| prefix.len() > best.len())
      {
        named = Some((prefix, seconds));
      }
    }
    match named {
      Some((_, seconds)) => seconds.as_f64(),
      None => rows.get("*").and_then(Number::as_f64),
    }
  }

  /// The cause of one request judged by this configuration's lifetime and prefix-reuse share.
  pub fn attribute(&self, request: &Request<'_>) -> Option<Cause> {
    attribute(request, self.ttl_for(request.mode, request.model), self.prefix_reuse_min)
  }

  /// The settings as now held (docs/api.md, Settings).
  pub fn settings(&self) -> Settings {
    let ttl_s = self.ttl_s.as_f64().unwrap_or(0.0);
    Settings {
      armed: self.armed,
      ttl_ms: (ttl_s * 1000.0).round().max(0.0) as u64,
      threshold: self.red_new_tokens,
    }
  }

  /// Apply a checked settings patch; the config keys it changed, with their new values, as they are
  /// merged into the config file.
  pub fn apply(&mut self, patch: &SettingsPatch) -> Map<String, Value> {
    let before = self.settings();
    let mut changed = Map::new();
    if let Some(armed) = patch.armed.filter(|&armed| armed != before.armed) {
      self.armed = armed;
      changed.insert("armed".into(), json!(armed));
    }
    if let Some(ttl_ms) = patch.ttl_ms.filter(|&ttl_ms| ttl_ms != before.ttl_ms) {
      self.ttl_s = seconds_of(ttl_ms);
      changed.insert("ttl_s".into(), Value::Number(self.ttl_s.clone()));
    }
    if let Some(threshold) = patch.threshold.filter(|&threshold| threshold != before.threshold) {
      self.red_new_tokens = threshold;
      changed.insert("red_new_tokens".into(), json!(threshold));
    }
    changed
  }
}

/// Milliseconds as the config file's seconds: an integer where they are whole.
fn seconds_of(ms: u64) -> Number {
  if ms.is_multiple_of(1000) {
    Number::from(ms / 1000)
  } else {
    Number::from_f64(ms as f64 / 1000.0).unwrap_or_else(|| Number::from(ms / 1000))
  }
}

impl Default for Config {
  fn default() -> Config {
    Config::defaults(&default_data_dir())
  }
}

/// Read the config file at `path` over `defaults`. A missing file is created with the defaults; a file
/// that cannot be read, or holds no object of the keys' types, leaves the defaults in force. Both are
/// logged.
pub fn load_config(path: &Path, defaults: Config, log: &Log) -> Config {
  let text = match fs::read_to_string(path) {
    Ok(text) => text,
    Err(why) if why.kind() == io::ErrorKind::NotFound => {
      match write_json(path, &serde_json::to_value(&defaults).unwrap_or_default()) {
        Ok(()) => log.info("config created with defaults", json!({"path": path})),
        Err(why) => log.error("config load failed — defaults", json!({"err": why.to_string()})),
      }
      return defaults;
    }
    Err(why) => {
      log.error("config load failed — defaults", json!({"err": why.to_string()}));
      return defaults;
    }
  };
  match over_defaults(&defaults, &text) {
    Ok(config) => config,
    Err(why) => {
      log.error("config load failed — defaults", json!({"err": why}));
      defaults
    }
  }
}

fn over_defaults(defaults: &Config, text: &str) -> Result<Config, String> {
  let Value::Object(file) = serde_json::from_str::<Value>(text).map_err(|why| why.to_string())? else {
    return Err("config file does not hold an object".into());
  };
  let Value::Object(mut merged) = serde_json::to_value(defaults).map_err(|why| why.to_string())? else {
    return Err("defaults are not an object".into());
  };
  merged.extend(file);
  serde_json::from_value(Value::Object(merged)).map_err(|why| why.to_string())
}

/// Merge only `updates` into the config file by an atomic replace; every other key stays as written. A
/// config path that is a symbolic link is written through: its final target is replaced, the temporary
/// file is made beside that target, and the link stays a link.
pub fn persist_config(path: &Path, updates: &Map<String, Value>) -> io::Result<()> {
  let target = final_target(path);
  let mut current = match fs::read_to_string(&target) {
    Ok(text) => match serde_json::from_str::<Value>(&text).map_err(io::Error::other)? {
      Value::Object(map) => map,
      _ => return Err(io::Error::other("config file does not hold an object")),
    },
    Err(why) if why.kind() == io::ErrorKind::NotFound => Map::new(),
    Err(why) => return Err(why),
  };
  for (key, value) in updates {
    current.insert(key.clone(), value.clone());
  }
  write_json(&target, &Value::Object(current))
}

/// Write `value` as indented JSON with a closing newline, through a temporary file beside `path` that
/// replaces it whole; the replaced file's permissions are kept.
fn write_json(path: &Path, value: &Value) -> io::Result<()> {
  let dir = match path.parent() {
    Some(dir) if !dir.as_os_str().is_empty() => dir,
    _ => Path::new("."),
  };
  let mut temporary = tempfile::Builder::new().prefix(".config-").suffix(".tmp").tempfile_in(dir)?;
  serde_json::to_writer_pretty(&mut temporary, value).map_err(io::Error::other)?;
  temporary.write_all(b"\n")?;
  if let Ok(meta) = fs::metadata(path) {
    fs::set_permissions(temporary.path(), meta.permissions())?;
  }
  temporary.persist(path).map_err(|failed| failed.error)?;
  Ok(())
}

/// The file a path finally names: the path resolved whole, or, where its target does not exist yet,
/// the end of its chain of links.
fn final_target(path: &Path) -> PathBuf {
  if let Ok(real) = fs::canonicalize(path) {
    return real;
  }
  let mut current = path.to_path_buf();
  for _ in 0..40 {
    let Ok(next) = fs::read_link(&current) else { break };
    current = match current.parent() {
      Some(dir) if next.is_relative() => dir.join(next),
      _ => next,
    };
  }
  current
}
