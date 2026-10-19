//! The configuration (docs/configuration.md): one JSON file read once at start over the defaults, and
//! the two TTL keys that `POST /ttl` changes live and merges back into that file (docs/api.md).

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Number, Value, json};

use crate::attribution::{Cause, Request, attribute};
use crate::log::Log;
use crate::paths::default_data_dir;

/// Per mode, a model prefix or `*` and the cache lifetime in seconds.
pub type TtlTable = IndexMap<String, IndexMap<String, Number>>;

/// The two keys that change while the daemon runs.
pub const TTL_KEYS: [&str; 2] = ["ttl_s", "ttl_by_mode_model_s"];

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
  pub ttl_s: Number,
  pub ttl_by_mode_model_s: TtlTable,
  pub prefix_reuse_min: f64,
  pub voice: bool,
  pub voice_cmd: Vec<String>,
  pub sound: bool,
  pub siren_file: String,
  pub quota_url: String,
  pub heartbeat_url: String,
  pub ring: usize,
  #[serde(serialize_with = "whole")]
  pub retention_days: f64,
  #[serde(serialize_with = "whole")]
  pub prune_every_s: f64,
  pub node: String,
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

impl Config {
  /// The defaults for a daemon whose data directory is `data_dir`, where the siren file is looked for.
  pub fn defaults(data_dir: &Path) -> Config {
    let mut ttl_by_mode_model_s = TtlTable::new();
    ttl_by_mode_model_s.insert("claude-code".into(), IndexMap::from([("*".into(), Number::from(3600))]));
    ttl_by_mode_model_s.insert("codex".into(), IndexMap::from([("gpt-5.6".into(), Number::from(1800))]));
    ttl_by_mode_model_s.insert("openrouter".into(), IndexMap::new());
    Config {
      port: 9022,
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
      ttl_s: Number::from(3600),
      ttl_by_mode_model_s,
      prefix_reuse_min: 0.5,
      voice: false,
      voice_cmd: Vec::new(),
      sound: true,
      siren_file: data_dir.join("warning.mp3").to_string_lossy().into_owned(),
      quota_url: "http://localhost:9200/api/claude".into(),
      heartbeat_url: String::new(),
      ring: 600,
      retention_days: 7.0,
      prune_every_s: 21_600.0,
      node: String::new(),
      extra: Map::new(),
    }
  }

  /// The cache lifetime in seconds of one request: within its mode's table the longest model prefix,
  /// else that mode's `*`; none for a listed mode with neither, since its provider has no uniform cache
  /// contract; `ttl_s` for a mode the table does not list.
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

  /// The two TTL keys as now held, as `GET /ttl` answers them.
  pub fn ttl(&self) -> Value {
    json!({"ttl_s": self.ttl_s, "ttl_by_mode_model_s": self.ttl_by_mode_model_s})
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

/// Why a TTL body was refused; `code` is the English error the caller gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TtlError {
  BodyNotJson,
  BodyNotObject,
  UnknownField,
  NothingToSet,
  TtlSNotPositiveInteger,
  TtlTableNotObject,
  TtlTableModeInvalid,
  TtlTableModelInvalid,
  TtlTableSecondsNotPositiveInteger,
}

impl TtlError {
  pub fn code(self) -> &'static str {
    match self {
      TtlError::BodyNotJson => "body_not_json",
      TtlError::BodyNotObject => "body_not_object",
      TtlError::UnknownField => "unknown_field",
      TtlError::NothingToSet => "nothing_to_set",
      TtlError::TtlSNotPositiveInteger => "ttl_s_not_positive_integer",
      TtlError::TtlTableNotObject => "ttl_table_not_object",
      TtlError::TtlTableModeInvalid => "ttl_table_mode_invalid",
      TtlError::TtlTableModelInvalid => "ttl_table_model_invalid",
      TtlError::TtlTableSecondsNotPositiveInteger => "ttl_table_seconds_not_positive_integer",
    }
  }
}

impl fmt::Display for TtlError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.code())
  }
}

impl std::error::Error for TtlError {}

/// A checked TTL body: the keys it gives, each to replace that key whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TtlUpdate {
  pub ttl_s: Option<Number>,
  pub ttl_by_mode_model_s: Option<TtlTable>,
}

impl TtlUpdate {
  /// Replace in `config` each key this update gives.
  pub fn apply(&self, config: &mut Config) {
    if let Some(ttl_s) = &self.ttl_s {
      config.ttl_s = ttl_s.clone();
    }
    if let Some(table) = &self.ttl_by_mode_model_s {
      config.ttl_by_mode_model_s = table.clone();
    }
  }

  /// The keys this update gives, as they are merged into the config file.
  pub fn to_map(&self) -> Map<String, Value> {
    let mut map = Map::new();
    if let Some(ttl_s) = &self.ttl_s {
      map.insert("ttl_s".into(), Value::Number(ttl_s.clone()));
    }
    if let Some(table) = &self.ttl_by_mode_model_s {
      map.insert("ttl_by_mode_model_s".into(), json!(table));
    }
    map
  }
}

/// Check a TTL body: an object with one or both TTL keys, `ttl_s` a positive integer of seconds and the
/// table a `{mode: {model prefix or "*": seconds}}` object with non-empty keys and positive integer
/// seconds. An empty mode table is allowed: that provider has no uniform cache contract.
pub fn ttl_body(body: &Value) -> Result<TtlUpdate, TtlError> {
  let Value::Object(body) = body else {
    return Err(TtlError::BodyNotObject);
  };
  if body.keys().any(|key| !TTL_KEYS.contains(&key.as_str())) {
    return Err(TtlError::UnknownField);
  }
  if body.is_empty() {
    return Err(TtlError::NothingToSet);
  }
  let mut update = TtlUpdate::default();
  if let Some(ttl_s) = body.get("ttl_s") {
    update.ttl_s = Some(positive_integer(ttl_s).ok_or(TtlError::TtlSNotPositiveInteger)?);
  }
  if let Some(table) = body.get("ttl_by_mode_model_s") {
    let Value::Object(table) = table else {
      return Err(TtlError::TtlTableNotObject);
    };
    let mut checked = TtlTable::new();
    for (mode, rows) in table {
      let Value::Object(rows) = rows else {
        return Err(TtlError::TtlTableModeInvalid);
      };
      if mode.is_empty() {
        return Err(TtlError::TtlTableModeInvalid);
      }
      let mut lifetimes = IndexMap::new();
      for (prefix, seconds) in rows {
        if prefix.is_empty() {
          return Err(TtlError::TtlTableModelInvalid);
        }
        lifetimes.insert(
          prefix.clone(),
          positive_integer(seconds).ok_or(TtlError::TtlTableSecondsNotPositiveInteger)?,
        );
      }
      checked.insert(mode.clone(), lifetimes);
    }
    update.ttl_by_mode_model_s = Some(checked);
  }
  Ok(update)
}

/// An integer above zero: never a boolean, a fraction or a negative.
fn positive_integer(value: &Value) -> Option<Number> {
  value.as_u64().filter(|&n| n > 0).map(Number::from)
}
