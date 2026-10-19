//! The engine: the transcripts it tails and their offsets, the sessions, the ring of rows and its durable
//! log, the burst window, the rolling five-hour buckets, the overlay state and the last quota reading.
//!
//! All of that mutable state sits behind one lock, so the scan, the posting callers and the readers of
//! `stats` never see it half-changed. Files are read and written outside it; the event log has a lock of
//! its own, shared by the append and the prune rewrite.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::attribution::Kind;
use crate::burst::{Recent, burst_summary};
use crate::clock::{now_epoch, now_local};
use crate::config::Config;
use crate::ctl::Ctl;
use crate::data_dir::DataDir;
use crate::event::{Event, Level, stamp, thousands};
use crate::log::{Log, LogLevel};
use crate::models::Backend;
use crate::posted::posted_event;
use crate::projects::{proj_dirs, project_of};
use crate::quota::QuotaReading;
use crate::session::Session;
use crate::transcript::{Origin, read_line};

/// One row of the ring: a row this run made, or one restored from the event log as it was written.
pub type Row = Map<String, Value>;

/// The rolling window the quota estimate sums over.
const ROLLING_S: f64 = 5.0 * 3600.0;
/// `stats` counts sessions with a request in this span.
const LIVE_S: f64 = 1800.0;

/// A red row handed to whatever sounds the siren.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
  pub kind: Kind,
  pub project: String,
  /// The session it belongs to; empty for a burst.
  pub sid: String,
  pub new: u64,
  pub note: String,
  /// Sound it whatever the cooldown.
  pub force: bool,
}

/// What sounds a red alert; `muted` is the overlay's state when it was raised.
pub trait AlertHook: Send + Sync {
  fn alert(&self, alert: &Alert, muted: bool);
}

/// The hook of an engine that sounds nothing.
pub struct Silent;

impl AlertHook for Silent {
  fn alert(&self, _alert: &Alert, _muted: bool) {}
}

/// Tokens of one minute, in total and per project.
#[derive(Clone, Debug, Default)]
struct Bucket {
  total: u64,
  by: IndexMap<String, u64>,
}

struct State {
  config: Config,
  sessions: HashMap<String, Session>,
  offsets: BTreeMap<String, u64>,
  ring: VecDeque<Row>,
  seq: u64,
  /// The requests of the burst window, by when they went out.
  recent: VecDeque<Recent>,
  last_burst_siren: f64,
  ctl: Ctl,
  quota: Option<QuotaReading>,
  /// Minute (Unix seconds / 60) → the Anthropic tokens of that minute.
  minute: BTreeMap<i64, Bucket>,
  files_watched: usize,
  last_event_ts: Option<String>,
}

impl State {
  fn push(&mut self, row: Row) {
    self.ring.push_back(row);
    while self.ring.len() > self.config.ring {
      self.ring.pop_front();
    }
  }

  fn add_bucket(&mut self, epoch: f64, project: &str, tokens: u64) {
    let bucket = self.minute.entry((epoch / 60.0).floor() as i64).or_default();
    bucket.total += tokens;
    *bucket.by.entry(project.into()).or_default() += tokens;
  }

  /// The tokens of the rolling five hours, in total and per project; older buckets are let go.
  fn rolling(&mut self, now: f64) -> (u64, IndexMap<String, u64>) {
    let cutoff = ((now - ROLLING_S) / 60.0).floor() as i64;
    self.minute = self.minute.split_off(&cutoff);
    let mut total = 0;
    let mut by: IndexMap<String, u64> = IndexMap::new();
    for bucket in self.minute.values() {
      total += bucket.total;
      for (project, tokens) in &bucket.by {
        *by.entry(project.clone()).or_default() += tokens;
      }
    }
    (total, by)
  }
}

pub struct Engine {
  state: Mutex<State>,
  events_file: Mutex<()>,
  data: DataDir,
  log: Arc<Log>,
  hook: Arc<dyn AlertHook>,
  seq_tx: watch::Sender<u64>,
  started: f64,
}

impl Engine {
  /// An engine over `data`, restoring the offsets, the overlay state and the newest `ring` rows of the
  /// event log; the restored rows are numbered afresh.
  pub fn new(config: Config, data: DataDir, log: Arc<Log>, hook: Arc<dyn AlertHook>) -> Engine {
    if let Err(why) = fs::create_dir_all(data.root()) {
      log.error("data directory not made", json!({"dir": data.root(), "err": why.to_string()}));
    }
    let offsets = read_json(&data.state())
      .and_then(|state| state.get("offsets").cloned())
      .and_then(|offsets| serde_json::from_value(offsets).ok())
      .unwrap_or_default();
    let mut ctl = Ctl::default();
    if let Some(Value::Object(saved)) = read_json(&data.ctl()) {
      ctl.merge(&saved);
    }
    let mut state = State {
      sessions: HashMap::new(),
      offsets,
      ring: VecDeque::new(),
      seq: 0,
      recent: VecDeque::new(),
      last_burst_siren: 0.0,
      ctl,
      quota: None,
      minute: BTreeMap::new(),
      files_watched: 0,
      last_event_ts: None,
      config,
    };
    for mut row in load_recent_events(&data.events(), state.config.ring) {
      state.seq += 1;
      row.insert("seq".into(), state.seq.into());
      state.push(row);
    }
    state.last_event_ts = state
      .ring
      .back()
      .and_then(|row| row.get("ts_kst"))
      .and_then(Value::as_str)
      .map(Into::into);
    let (seq_tx, _) = watch::channel(state.seq);
    Engine {
      state: Mutex::new(state),
      events_file: Mutex::new(()),
      data,
      log,
      hook,
      seq_tx,
      started: now_epoch(),
    }
  }

  fn state(&self) -> MutexGuard<'_, State> {
    self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  pub fn log(&self) -> &Log {
    &self.log
  }

  pub fn data_dir(&self) -> &DataDir {
    &self.data
  }

  /// A copy of the configuration as now held.
  pub fn config(&self) -> Config {
    self.state().config.clone()
  }

  /// Change the configuration as now held, under the one lock.
  pub fn with_config<T>(&self, change: impl FnOnce(&mut Config) -> T) -> T {
    change(&mut self.state().config)
  }

  /// The newest sequence number, which moves whenever a row is added.
  pub fn subscribe(&self) -> watch::Receiver<u64> {
    self.seq_tx.subscribe()
  }

  pub fn seq(&self) -> u64 {
    self.state().seq
  }

  /// A copy of the ring, oldest first.
  pub fn rows(&self) -> Vec<Row> {
    self.state().ring.iter().cloned().collect()
  }

  pub fn ctl(&self) -> Ctl {
    self.state().ctl.clone()
  }

  /// Change the overlay state and keep it in the data directory; the state before and after.
  pub fn set_ctl(&self, body: &Map<String, Value>) -> (Ctl, Ctl) {
    let (before, after) = {
      let mut state = self.state();
      let before = state.ctl.clone();
      state.ctl.merge(body);
      (before, state.ctl.clone())
    };
    self.save_ctl(&after);
    (before, after)
  }

  pub fn set_quota(&self, reading: QuotaReading) {
    self.state().quota = Some(reading);
  }

  // ---- persistence

  /// Keep the transcript offsets, by an atomic replace.
  pub fn save(&self) {
    let offsets = self.state().offsets.clone();
    if let Err(why) = replace_file(&self.data.state(), json!({"offsets": offsets}).to_string().as_bytes()) {
      self.log.error("state save failed", json!({"err": why.to_string()}));
    }
  }

  fn save_ctl(&self, ctl: &Ctl) {
    if let Ok(text) = serde_json::to_string(ctl) {
      let _ = fs::write(self.data.ctl(), text);
    }
  }

  // ---- scanning

  /// The transcripts modified within `recent_hours`: sessions and their subagents, under every root.
  pub fn files(&self) -> Vec<PathBuf> {
    let config = self.config();
    let horizon = SystemTime::now()
      .checked_sub(std::time::Duration::from_secs_f64(config.recent_hours.max(0.0) * 3600.0))
      .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut found = Vec::new();
    for root in proj_dirs(&config) {
      let root = glob::Pattern::escape(&root.to_string_lossy());
      for pattern in ["*/*.jsonl", "*/*/subagents/*.jsonl"] {
        let Ok(paths) = glob::glob(&format!("{root}/{pattern}")) else { continue };
        for path in paths.filter_map(Result::ok) {
          if fs::metadata(&path).and_then(|meta| meta.modified()).is_ok_and(|at| at >= horizon) {
            found.push(path);
          }
        }
      }
    }
    found
  }

  /// One pass of the scan loop: tail every recent transcript; the number of files watched.
  pub fn scan_all(&self) -> usize {
    let files = self.files();
    self.state().files_watched = files.len();
    for path in &files {
      self.scan(path, false, 0.0);
    }
    files.len()
  }

  /// At start, read the transcripts modified within `backfill_hours` from their beginning, adding only
  /// rows inside that span and silently, and skip every other unread transcript to its end; then order
  /// the ring by time and number it afresh.
  pub fn backfill(&self) {
    let horizon = now_epoch() - self.config().backfill_hours * 3600.0;
    let mut added = 0;
    for path in self.files() {
      let key = path.to_string_lossy().into_owned();
      if self.state().offsets.contains_key(&key) {
        continue;
      }
      let Ok(meta) = fs::metadata(&path) else { continue };
      if modified_epoch(&meta) >= horizon {
        self.state().offsets.insert(key, 0);
        added += self.scan(&path, true, horizon);
      } else {
        self.state().offsets.insert(key, meta.len());
      }
    }
    let files = {
      let mut state = self.state();
      let mut rows: Vec<Row> = state.ring.drain(..).collect();
      rows.sort_by(|a, b| epoch_in(a).total_cmp(&epoch_in(b)));
      state.seq = 0;
      for mut row in rows {
        state.seq += 1;
        row.insert("seq".into(), state.seq.into());
        state.push(row);
      }
      self.seq_tx.send_replace(state.seq);
      state.offsets.len()
    };
    self.save();
    self.log.info("backfill done", json!({"events": added, "files": files}));
  }

  /// Without a backfill: every recent transcript not read before is taken from its end.
  pub fn skip_unread(&self) {
    let files = self.files();
    let mut state = self.state();
    for path in files {
      if let Ok(meta) = fs::metadata(&path) {
        state.offsets.entry(path.to_string_lossy().into_owned()).or_insert(meta.len());
      }
    }
  }

  /// Read the lines added to one transcript since its offset, up to the last whole line, and add the rows
  /// they make at or after `min_epoch`; `quiet` adds them without the burst window or an alert. The
  /// number of rows added.
  pub fn scan(&self, path: &Path, quiet: bool, min_epoch: f64) -> usize {
    let key = path.to_string_lossy().into_owned();
    let Ok(size) = fs::metadata(path).map(|meta| meta.len()) else { return 0 };
    let mut offset = self.state().offsets.get(&key).copied().unwrap_or(0);
    if size < offset {
      offset = 0;
    }
    if size == offset {
      return 0;
    }
    let Ok(data) = read_from(path, offset) else { return 0 };
    let whole = data.iter().rposition(|&byte| byte == b'\n').map_or(0, |at| at + 1);
    let data = &data[..whole];
    self.state().offsets.insert(key, offset + whole as u64);

    let config = self.config();
    let owner = project_of(path, &config);
    let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let origin = Origin {
      sid: file_name.strip_suffix(".jsonl").unwrap_or(&file_name).into(),
      project: owner.project,
      is_agent: owner.is_agent,
      parent: owner.parent,
    };
    let mut added = 0;
    for raw in data.split(|&byte| byte == b'\n').filter(|raw| !raw.is_empty()) {
      let Ok(line) = serde_json::from_slice::<Value>(raw) else { continue };
      if let Some(event) = self.handle(&line, &origin)
        && event.epoch >= min_epoch
      {
        self.emit(event, quiet);
        added += 1;
      }
    }
    added
  }

  /// Read one parsed transcript line into its session; the row it makes, if any.
  pub fn handle(&self, line: &Value, origin: &Origin) -> Option<Event> {
    let mut state = self.state();
    let State {
      sessions, config, ..
    } = &mut *state;
    let session = sessions.entry(origin.sid.clone()).or_default();
    read_line(line, origin, session, config)
  }

  /// Check one posted request (docs/api.md, `POST /events`) and make its row; a session is seen once a row
  /// of the same mode, session and model is in the ring.
  pub fn external(&self, body: &Value) -> Result<Event, String> {
    let state = self.state();
    let seen = |mode: &str, sid: &str, model: &str| {
      let is = |row: &Row, key: &str, want: &str| row.get(key).and_then(Value::as_str) == Some(want);
      state
        .ring
        .iter()
        .any(|row| is(row, "mode", mode) && is(row, "sid", sid) && is(row, "model", model))
    };
    posted_event(body, seen, &state.config, &now_local())
  }

  // ---- emit

  /// Add a row: number it into the ring, append it to the event log and log a notable one. Unless
  /// `quiet`, it also enters the burst window and the rolling buckets, a red row is handed to the alert
  /// hook, and a burst past `burst_red_tokens` adds a burst row and its alert. The row's sequence number.
  pub fn emit(&self, event: Event, quiet: bool) -> u64 {
    let now = now_epoch();
    let notable = matches!(event.level, Level::Yellow | Level::Red)
      || matches!(event.kind, Kind::Refusal | Kind::Quota | Kind::Cmd | Kind::Compact);
    let summary = notable.then(|| summary_line(&event));
    let red = (event.level == Level::Red).then(|| Alert {
      kind: event.kind,
      project: event.project.clone(),
      sid: event.sid.clone(),
      new: event.new,
      note: event.note.clone(),
      force: false,
    });
    let (backend, new, epoch, project, read) =
      (event.backend, event.new, event.epoch, event.project.clone(), event.read);

    let mut alerts = Vec::new();
    let (seq, row, muted) = {
      let mut state = self.state();
      state.seq += 1;
      let seq = state.seq;
      let row = event.into_row(seq);
      state.last_event_ts = row.get("ts_kst").and_then(Value::as_str).map(Into::into);
      state.push(row.clone());
      if !quiet {
        let at = if epoch > 0.0 { epoch } else { now };
        if backend != Backend::Local && new > 0 {
          // The window is measured by when the request went out: by when the line was read, a
          // transcript written late, a finished subagent flushing at once, would pack requests from
          // different minutes into one window.
          state.recent.push_back(Recent {
            at,
            new,
            project: project.clone(),
            read,
          });
        }
        if backend == Backend::Anthropic && new > 0 {
          let owner = if project.is_empty() { "?" } else { project.as_str() };
          state.add_bucket(at, owner, new);
        }
        let window = state.config.burst_window_s;
        state.recent.retain(|request| request.at >= now - window);
        alerts.extend(red);
        let burst = burst_summary(&state.recent);
        if burst.new >= state.config.burst_red_tokens && now - state.last_burst_siren > window {
          state.last_burst_siren = now;
          let owners: Vec<&str> = burst.parts.iter().take(4).map(|part| part.project.as_str()).collect();
          let owners = owners.join(",");
          alerts.push(Alert {
            kind: Kind::Burst,
            project: owners.clone(),
            sid: String::new(),
            new: burst.new,
            note: format!("{} req/{}s", burst.req, whole_number(window)),
            force: true,
          });
          state.seq += 1;
          let (ts_kst, date_kst, _) = stamp(&now_local());
          let burst_row = json!({
            "seq": state.seq, "ts_kst": ts_kst, "date_kst": date_kst, "epoch": now,
            "kind": "burst", "level": "red", "project": owners, "session": "", "m": "",
            "new": burst.new, "read": burst.read, "note": format!("{} req in {}s", burst.req, whole_number(window)),
            "parts": burst.parts, "req": "", "backend": "anthropic", "cost": 0,
          });
          if let Value::Object(burst_row) = burst_row {
            state.push(burst_row);
          }
        }
      }
      self.seq_tx.send_replace(state.seq);
      (seq, row, state.ctl.muted)
    };

    if let Err(why) = self.append(&row) {
      self.log.error("events append failed", json!({"err": why.to_string()}));
    }
    if let Some((level, message, fields)) = summary {
      self.log.line(level, &message, fields);
    }
    for alert in &alerts {
      self.hook.alert(alert, muted);
    }
    seq
  }

  /// Hand one alert to the hook with the overlay's muted state; that state.
  pub fn alert(&self, alert: &Alert) -> bool {
    let muted = self.state().ctl.muted;
    self.hook.alert(alert, muted);
    muted
  }

  fn append(&self, row: &Row) -> io::Result<()> {
    let _held = self.events_file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = OpenOptions::new().create(true).append(true).open(self.data.events())?;
    file.write_all(format!("{}\n", Value::Object(row.clone())).as_bytes())
  }

  // ---- the event log

  /// Drop the event-log rows older than `retention_days`, by an atomic rewrite; 0 keeps everything.
  pub fn prune_events(&self) {
    let days = self.config().retention_days;
    let path = self.data.events();
    if days <= 0.0 || !path.exists() {
      return;
    }
    let horizon = now_epoch() - days * 86_400.0;
    let outcome = {
      let _held = self.events_file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      prune_file(&path, horizon)
    };
    match outcome {
      Ok((removed, kept)) if removed > 0 => {
        self
          .log
          .info("events pruned", json!({"removed": removed, "kept": kept, "retention_days": days}));
      }
      Ok(_) => {}
      Err(why) => self.log.error("prune failed", json!({"err": why.to_string()})),
    }
  }

  /// At start, fill the rolling buckets from the Anthropic rows of the event log's last five hours.
  pub fn seed_buckets(&self) {
    let cutoff = now_epoch() - ROLLING_S;
    let mut seeded = 0;
    let text = fs::read(self.data.events()).unwrap_or_default();
    let mut state = self.state();
    for raw in text.split(|&byte| byte == b'\n') {
      let Ok(Value::Object(row)) = serde_json::from_slice::<Value>(raw) else { continue };
      let epoch = epoch_in(&row);
      if row.get("backend").and_then(Value::as_str) == Some("anthropic") && epoch >= cutoff {
        let project = row.get("project").and_then(Value::as_str).filter(|p| !p.is_empty()).unwrap_or("?");
        let new = row.get("new").and_then(Value::as_u64).unwrap_or(0);
        state.add_bucket(epoch, project, new);
        seeded += 1;
      }
    }
    let (total, _) = state.rolling(now_epoch());
    drop(state);
    self.log.info(
      "rolling 5h buckets seeded from events.jsonl",
      json!({"events": seeded, "window_tokens": total}),
    );
  }

  // ---- stats

  /// The `GET /stats` object (docs/api.md).
  pub fn stats(&self) -> Value {
    let now = now_epoch();
    let mut state = self.state();
    let mut per_min = [0u64; 10];
    let mut live = std::collections::HashSet::new();
    for row in &state.ring {
      let local = row.get("backend").and_then(Value::as_str) == Some("local");
      let epoch = row.get("epoch").and_then(Value::as_f64);
      let new = row.get("new").and_then(Value::as_u64).unwrap_or(0);
      if !local && new > 0 {
        let age = now - epoch.unwrap_or(now);
        if (0.0..600.0).contains(&age) {
          per_min[9 - (age / 60.0) as usize] += new;
        }
      }
      if let Some(sid) = row.get("sid").and_then(Value::as_str).filter(|sid| !sid.is_empty())
        && !local
        && now - epoch.unwrap_or(0.0) < LIVE_S
      {
        live.insert(sid.to_string());
      }
    }
    let new_60s: u64 = state
      .recent
      .iter()
      .filter(|request| request.at > now - 60.0)
      .map(|request| request.new)
      .sum();

    let (total, by) = state.rolling(now);
    let mut quota = match &state.quota {
      Some(reading) => match serde_json::to_value(reading) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
      },
      None => Map::new(),
    };
    let used = quota.get("five_h").cloned().unwrap_or(Value::Null);
    let util = used.as_f64();
    let rate = util
      .filter(|&util| util != 0.0 && total > 0)
      .map(|util| round(util / (total as f64 / 1e6), 3));
    let mut segments: Vec<(String, u64)> = by.into_iter().collect();
    segments.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    let segments: Vec<Value> = match util {
      Some(util) if total > 0 => segments
        .into_iter()
        .take(12)
        .map(|(project, tokens)| json!({"project": project, "tokens": tokens, "pct": round(tokens as f64 / total as f64 * util, 2)}))
        .collect(),
      _ => Vec::new(),
    };
    quota.insert("pct_per_mtok".into(), json!(rate));
    quota.insert("window_tokens".into(), json!(total));
    quota.insert(
      "budget".into(),
      json!({"used_pct": if util.is_some() { used } else { Value::Null },
             "headroom_pct": util.map(|util| round(100.0 - util, 1)),
             "segments": segments, "rolling_5h": true}),
    );
    let config = &state.config;
    json!({
      "per_min": per_min,
      "new_60s": new_60s,
      "sessions_30m": live.len(),
      "quota": quota,
      "uptime_s": (now - self.started) as i64,
      "files": state.files_watched,
      "last_event": state.last_event_ts,
      "ctl": state.ctl,
      "cfg": {"yellow_new_tokens": config.yellow_new_tokens, "red_new_tokens": config.red_new_tokens,
              "burst_red_tokens": config.burst_red_tokens},
    })
  }
}

/// The newest `limit` rows of the event log that read as objects; a broken line is skipped and does not
/// cost the rest.
pub fn load_recent_events(path: &Path, limit: usize) -> Vec<Row> {
  if limit == 0 {
    return Vec::new();
  }
  let Ok(text) = fs::read(path) else { return Vec::new() };
  let mut rows = VecDeque::with_capacity(limit);
  for raw in text.split(|&byte| byte == b'\n') {
    if let Ok(Value::Object(row)) = serde_json::from_slice::<Value>(raw) {
      if rows.len() == limit {
        rows.pop_front();
      }
      rows.push_back(row);
    }
  }
  rows.into()
}

/// A notable row's log line: its level, its message and its fields.
fn summary_line(event: &Event) -> (LogLevel, String, Value) {
  let level = if event.level == Level::Red { LogLevel::Warn } else { LogLevel::Info };
  let message = format!(
    "{} {}/{} {} +{} read={} {} «{}»",
    event.kind.as_str().to_uppercase(),
    event.project,
    event.session,
    event.m,
    thousands(event.new),
    thousands(event.read),
    event.note,
    event.req
  );
  (level, message, json!({"lvl": event.level, "cost": event.cost, "agent": event.agent}))
}

fn epoch_in(row: &Row) -> f64 {
  row.get("epoch").and_then(Value::as_f64).unwrap_or(0.0)
}

fn modified_epoch(meta: &fs::Metadata) -> f64 {
  meta
    .modified()
    .ok()
    .and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok())
    .map_or(0.0, |since| since.as_secs_f64())
}

/// Seconds as the burst note writes them: whole seconds without a fraction.
fn whole_number(seconds: f64) -> String {
  if seconds.fract() == 0.0 { format!("{}", seconds as i64) } else { seconds.to_string() }
}

fn round(value: f64, places: i32) -> f64 {
  let scale = 10f64.powi(places);
  (value * scale).round() / scale
}

fn read_json(path: &Path) -> Option<Value> {
  serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn read_from(path: &Path, offset: u64) -> io::Result<Vec<u8>> {
  let mut file = fs::File::open(path)?;
  file.seek(SeekFrom::Start(offset))?;
  let mut data = Vec::new();
  file.read_to_end(&mut data)?;
  Ok(data)
}

/// Replace a file whole with `contents`, through a temporary file beside it.
fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
  let dir = path.parent().unwrap_or(Path::new("."));
  let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
  temporary.write_all(contents)?;
  temporary.persist(path).map_err(|failed| failed.error)?;
  Ok(())
}

/// Keep the event-log lines at or after `horizon`; a line that is no row counts as oldest. Rewritten
/// only when something goes; the number removed and kept.
fn prune_file(path: &Path, horizon: f64) -> io::Result<(usize, usize)> {
  let text = fs::read(path)?;
  let mut kept = Vec::new();
  let mut removed = 0;
  let mut kept_count = 0;
  for raw in text.split(|&byte| byte == b'\n') {
    if raw.iter().all(u8::is_ascii_whitespace) {
      continue;
    }
    let epoch = match serde_json::from_slice::<Value>(raw) {
      Ok(Value::Object(row)) => epoch_in(&row),
      _ => 0.0,
    };
    if epoch >= horizon {
      kept.extend_from_slice(raw);
      kept.push(b'\n');
      kept_count += 1;
    } else {
      removed += 1;
    }
  }
  if removed > 0 {
    replace_file(path, &kept)?;
  }
  Ok((removed, kept_count))
}
