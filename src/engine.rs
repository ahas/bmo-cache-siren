//! The engine: the transcripts it tails and their offsets, the sessions, the event window and its durable
//! log, the burst window and the settings (docs/api.md, Events).
//!
//! All of that mutable state sits behind one lock, so the scan, the posting callers and the readers never
//! see it half-changed. Files are read and written outside it; the event log has a lock of its own,
//! shared by the append and the prune rewrite.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::attribution::Kind;
use crate::burst::{BurstPart, Recent, burst_summary};
use crate::clock::now_millis;
use crate::config::Config;
use crate::data_dir::DataDir;
use crate::event::{Level, thousands};
use crate::log::{Log, LogLevel};
use crate::models::Backend;
use crate::posted::{Previous, posted_usage};
use crate::projects::{proj_dirs, project_of};
use crate::session::Session;
use crate::transcript::{Origin, read_line};
use crate::wire::{
  Event, EventPage, EventQuery, Happening, NewUsage, SessionStats, Settings, SettingsPatch, Siren,
  SirenPart, Stats, StatsQuery, Usage,
};

/// The page's size where `limit` is absent, and the most it may ask.
pub const DEFAULT_LIMIT: u32 = 100;
pub const MOST_LIMIT: u32 = 1000;

/// A red request or a burst handed to whatever sounds the siren.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
  pub kind: Kind,
  pub project: String,
  /// The session it belongs to; empty for a burst.
  pub sid: String,
  pub new: u64,
  pub read: u64,
  pub requests: u64,
  pub note: String,
  /// A burst's projects, largest first.
  pub parts: Vec<BurstPart>,
  /// Sound it whatever the cooldown.
  pub force: bool,
}

/// What sounds an alert. None where its cooldown keeps it quiet; else whether sound or voice went out,
/// which only an `armed` siren lets them.
pub trait AlertHook: Send + Sync {
  fn alert(&self, alert: &Alert, armed: bool) -> Option<bool>;
}

/// The hook of an engine that raises every alert and sounds none.
pub struct Silent;

impl AlertHook for Silent {
  fn alert(&self, _alert: &Alert, _armed: bool) -> Option<bool> {
    Some(false)
  }
}

/// Why a request was refused, with the code the route answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
  BadRequest(String),
  Conflict(String),
  Gone(String),
}

struct State {
  config: Config,
  sessions: HashMap<String, Session>,
  offsets: BTreeMap<String, u64>,
  /// The newest `ring` events, oldest first.
  window: VecDeque<Event>,
  /// The number the next event takes.
  next_id: i64,
  /// The requests of the burst window, by when they went out.
  recent: VecDeque<Recent>,
  last_burst_ms: i64,
  files_watched: usize,
}

impl State {
  /// Number an event and keep it in the window.
  fn record(&mut self, at: i64, happening: Happening) -> Event {
    let event = Event {
      id: self.next_id,
      at,
      happening,
    };
    self.next_id += 1;
    self.window.push_back(event.clone());
    while self.window.len() > self.config.ring.max(1) {
      self.window.pop_front();
    }
    event
  }

  /// The number of the first event the window holds, or of the next one where it holds none.
  fn window_start(&self) -> i64 {
    self.window.front().map_or(self.next_id, |event| event.id)
  }

  fn usages(&self) -> impl Iterator<Item = &Usage> {
    self.window.iter().filter_map(|event| match &event.happening {
      Happening::Usage(usage) => Some(usage.as_ref()),
      _ => None,
    })
  }
}

pub struct Engine {
  state: Mutex<State>,
  events_file: Mutex<()>,
  data: DataDir,
  log: Arc<Log>,
  hook: Arc<dyn AlertHook>,
  newest: watch::Sender<i64>,
}

impl Engine {
  /// An engine over `data`, restoring the offsets and the newest `ring` events of the event log; the
  /// numbering carries on past the highest number in the log.
  pub fn new(config: Config, data: DataDir, log: Arc<Log>, hook: Arc<dyn AlertHook>) -> Engine {
    if let Err(why) = fs::create_dir_all(data.root()) {
      log.error("data directory not made", json!({"dir": data.root(), "err": why.to_string()}));
    }
    let offsets = read_json(&data.state())
      .and_then(|state| state.get("offsets").cloned())
      .and_then(|offsets| serde_json::from_value(offsets).ok())
      .unwrap_or_default();
    let (window, highest) = load_window(&data.events(), config.ring);
    let state = State {
      sessions: HashMap::new(),
      offsets,
      window,
      next_id: highest + 1,
      recent: VecDeque::new(),
      last_burst_ms: 0,
      files_watched: 0,
      config,
    };
    let (newest, _) = watch::channel(highest);
    Engine {
      state: Mutex::new(state),
      events_file: Mutex::new(()),
      data,
      log,
      hook,
      newest,
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

  /// The number of the newest event, which moves whenever one is recorded.
  pub fn subscribe(&self) -> watch::Receiver<i64> {
    self.newest.subscribe()
  }

  /// A copy of the window, oldest first.
  pub fn window(&self) -> Vec<Event> {
    self.state().window.iter().cloned().collect()
  }

  /// How many transcripts the last scan watched.
  pub fn files_watched(&self) -> usize {
    self.state().files_watched
  }

  // ---- persistence

  /// Keep the transcript offsets, by an atomic replace.
  pub fn save(&self) {
    let offsets = self.state().offsets.clone();
    if let Err(why) = replace_file(&self.data.state(), json!({"offsets": offsets}).to_string().as_bytes()) {
      self.log.error("state save failed", json!({"err": why.to_string()}));
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

  /// One pass of the scan loop: tail every recent transcript and record what it says; the number of
  /// files watched.
  pub fn scan_all(&self) -> usize {
    let files = self.files();
    self.state().files_watched = files.len();
    for path in &files {
      for usage in self.scan(path, i64::MIN) {
        self.record_usage(usage, false);
      }
    }
    files.len()
  }

  /// At start, read the transcripts modified within `backfill_hours` from their beginning, recording
  /// only the Usages inside that span, silently and in time order, and skip every other unread
  /// transcript to its end.
  pub fn backfill(&self) {
    let horizon = now_millis() - (self.config().backfill_hours * 3_600_000.0) as i64;
    let mut found = Vec::new();
    for path in self.files() {
      let key = path.to_string_lossy().into_owned();
      if self.state().offsets.contains_key(&key) {
        continue;
      }
      let Ok(meta) = fs::metadata(&path) else { continue };
      if modified_millis(&meta) >= horizon {
        self.state().offsets.insert(key, 0);
        found.extend(self.scan(&path, horizon));
      } else {
        self.state().offsets.insert(key, meta.len());
      }
    }
    found.sort_by_key(|usage| usage.at);
    let added = found.len();
    for usage in found {
      self.record_usage(usage, true);
    }
    self.save();
    let files = self.state().offsets.len();
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

  /// Read the lines added to one transcript since its offset, up to the last whole line; the Usages they
  /// make at or after `min_at`, in the order of the file.
  pub fn scan(&self, path: &Path, min_at: i64) -> Vec<Usage> {
    let key = path.to_string_lossy().into_owned();
    let Ok(size) = fs::metadata(path).map(|meta| meta.len()) else { return Vec::new() };
    let mut offset = self.state().offsets.get(&key).copied().unwrap_or(0);
    if size < offset {
      offset = 0;
    }
    if size == offset {
      return Vec::new();
    }
    let Ok(data) = read_from(path, offset) else { return Vec::new() };
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
    let mut found = Vec::new();
    for raw in data.split(|&byte| byte == b'\n').filter(|raw| !raw.is_empty()) {
      let Ok(line) = serde_json::from_slice::<Value>(raw) else { continue };
      if let Some(usage) = self.handle(&line, &origin)
        && usage.at >= min_at
      {
        found.push(usage);
      }
    }
    found
  }

  /// Read one parsed transcript line into its session; the Usage it makes, if any.
  pub fn handle(&self, line: &Value, origin: &Origin) -> Option<Usage> {
    let mut state = self.state();
    let State {
      sessions, config, ..
    } = &mut *state;
    let session = sessions.entry(origin.sid.clone()).or_default();
    read_line(line, origin, session, config)
  }

  /// Check one posted request (docs/api.md, `POST /events`) and make its Usage, read against what the
  /// window holds of its session.
  pub fn posted(&self, body: NewUsage) -> Result<Usage, String> {
    let state = self.state();
    let harness = body.harness_name.as_deref().unwrap_or("").trim();
    let session = body.session_id.trim();
    let model = body.model_name.trim();
    let mut previous = Previous::default();
    for usage in state.usages().filter(|usage| usage.session_id == session) {
      previous.seen |= usage.harness_name == harness && usage.model_name == model;
      previous.at = Some(usage.at);
      previous.context_token_count = usage.context_token_count;
    }
    posted_usage(body, previous, &state.config)
  }

  // ---- recording

  /// Record a Usage: number it into the window, append it to the event log and log a notable one. Unless
  /// `quiet`, it also enters the burst window, a red Usage is handed to the alert hook, and a burst past
  /// `burst_red_tokens` raises a burst alert; each alert raised is recorded as a Siren. The Usage's
  /// event.
  pub fn record_usage(&self, usage: Usage, quiet: bool) -> Event {
    let now = now_millis();
    let notable = matches!(usage.level, Level::Yellow | Level::Red)
      || matches!(usage.kind, Kind::Refusal | Kind::Quota | Kind::Cmd | Kind::Compact);
    let summary = notable.then(|| summary_line(&usage));
    let red = (usage.level == Level::Red).then(|| Alert {
      kind: usage.kind,
      project: usage.project_name.clone(),
      sid: usage.session_id.clone(),
      new: usage.new_token_count,
      read: usage.cached_token_count,
      requests: 1,
      note: usage.note.clone(),
      parts: Vec::new(),
      force: false,
    });
    let (backend, new, at, project, read) = (
      usage.backend,
      usage.new_token_count,
      usage.at,
      usage.project_name.clone(),
      usage.cached_token_count,
    );

    let mut alerts = Vec::new();
    let (event, armed) = {
      let mut state = self.state();
      let event = state.record(at, Happening::Usage(Box::new(usage)));
      if !quiet {
        if backend != Backend::Local && new > 0 {
          // The window is measured by when the request went out: by when the line was read, a
          // transcript written late, a finished subagent flushing at once, would pack requests from
          // different minutes into one window.
          state.recent.push_back(Recent {
            at: at as f64 / 1000.0,
            new,
            project,
            read,
          });
        }
        let window_ms = (state.config.burst_window_s * 1000.0) as i64;
        let cutoff = (now - window_ms) as f64 / 1000.0;
        state.recent.retain(|request| request.at >= cutoff);
        alerts.extend(red);
        let burst = burst_summary(&state.recent);
        if burst.new >= state.config.burst_red_tokens && now - state.last_burst_ms > window_ms {
          state.last_burst_ms = now;
          let owners: Vec<&str> = burst.parts.iter().take(4).map(|part| part.project.as_str()).collect();
          alerts.push(Alert {
            kind: Kind::Burst,
            project: owners.join(","),
            sid: String::new(),
            new: burst.new,
            read: burst.read,
            requests: burst.req,
            note: format!("{} req in {}s", burst.req, whole_number(state.config.burst_window_s)),
            parts: burst.parts,
            force: true,
          });
        }
      }
      (event, state.config.armed)
    };
    self.keep(&event);
    if let Some((level, message, fields)) = summary {
      self.log.line(level, &message, fields);
    }
    for alert in &alerts {
      if let Some(audible) = self.hook.alert(alert, armed) {
        self.record(Happening::Siren(siren_of(alert, now, audible)));
      }
    }
    event
  }

  /// Number an event recorded now into the window and keep it in the event log.
  pub fn record(&self, happening: Happening) -> Event {
    let event = self.state().record(now_millis(), happening);
    self.keep(&event);
    event
  }

  /// Append an event to the event log and tell the readers waiting on the stream.
  fn keep(&self, event: &Event) {
    if let Err(why) = self.append(event) {
      self.log.error("events append failed", json!({"err": why.to_string()}));
    }
    self.newest.send_replace(event.id);
  }

  /// Sound the siren once as a red cold alert, whatever the cooldown; whether sound or voice went out.
  pub fn test_siren(&self) -> bool {
    let armed = self.state().config.armed;
    let alert = Alert {
      kind: Kind::Cold,
      project: "test".into(),
      sid: "test".into(),
      new: 123_456,
      read: 0,
      requests: 1,
      note: "manual test".into(),
      parts: Vec::new(),
      force: true,
    };
    self.hook.alert(&alert, armed).unwrap_or(false)
  }

  fn append(&self, event: &Event) -> io::Result<()> {
    let line = serde_json::to_string(event).map_err(io::Error::other)?;
    let _held = self.events_file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = OpenOptions::new().create(true).append(true).open(self.data.events())?;
    file.write_all(format!("{line}\n").as_bytes())
  }

  // ---- the settings

  pub fn settings(&self) -> Settings {
    self.state().config.settings()
  }

  /// Check and apply a settings patch: the settings after it, and the config keys it changed as they are
  /// merged into the config file. A change records a `settings.changed` event.
  pub fn change_settings(&self, patch: &SettingsPatch) -> Result<(Settings, Map<String, Value>), Refusal> {
    if patch.ttl_ms == Some(0) {
      return Err(Refusal::BadRequest("ttlMs must be above 0".into()));
    }
    if patch.threshold == Some(0) {
      return Err(Refusal::BadRequest("threshold must be above 0".into()));
    }
    let (settings, changed) = {
      let mut state = self.state();
      let yellow = state.config.yellow_new_tokens;
      if let Some(threshold) = patch.threshold
        && threshold <= yellow
      {
        return Err(Refusal::Conflict(format!(
          "threshold must stay above the yellow line, {yellow} new tokens"
        )));
      }
      let changed = state.config.apply(patch);
      (state.config.settings(), changed)
    };
    if !changed.is_empty() {
      self.record(Happening::Settings(settings));
    }
    Ok((settings, changed))
  }

  // ---- reading

  /// The events a page answers (docs/api.md, `GET /events`).
  pub fn page(&self, query: &EventQuery) -> Result<EventPage, Refusal> {
    let limit = limit_of(query.limit)?;
    let filter = Filter::of(query)?;
    let state = self.state();
    if let Some(after) = query.after {
      gone_check(&state, after)?;
      let mut matching = state.window.iter().filter(|event| event.id > after && filter.passes(event));
      let items: Vec<Event> = matching.by_ref().take(limit).cloned().collect();
      let more = matching.next().is_some();
      return Ok(EventPage { items, more });
    }
    let matching: Vec<&Event> = state.window.iter().filter(|event| filter.passes(event)).collect();
    let skip = matching.len().saturating_sub(limit);
    let items = matching.into_iter().skip(skip).cloned().collect();
    Ok(EventPage { items, more: false })
  }

  /// Where a stream starts: after `after`, or after the newest event where none is given.
  pub fn stream_start(&self, after: Option<i64>) -> Result<i64, Refusal> {
    let state = self.state();
    match after {
      Some(after) => {
        gone_check(&state, after)?;
        Ok(after)
      }
      None => Ok(state.next_id - 1),
    }
  }

  /// The events past `after` that `query` lets through, oldest first.
  pub fn events_after(&self, after: i64, filter: &Filter) -> Vec<Event> {
    let state = self.state();
    state
      .window
      .iter()
      .filter(|event| event.id > after && filter.passes(event))
      .cloned()
      .collect()
  }

  /// The window's Usages summed by session (docs/api.md, Stats).
  pub fn stats(&self, query: &StatsQuery) -> Result<Stats, Refusal> {
    if query.since.is_some_and(|since| since < 0) {
      return Err(Refusal::BadRequest("since must be epoch milliseconds, at least 0".into()));
    }
    let state = self.state();
    let since = query
      .since
      .unwrap_or_else(|| state.window.iter().map(|event| event.at).min().unwrap_or_else(now_millis));
    let wanted =
      |session: Option<&str>| query.session_id.as_deref().is_none_or(|want| session == Some(want));
    let mut sessions: Vec<SessionStats> = Vec::new();
    let mut siren_count = 0;
    for event in state.window.iter().filter(|event| event.at >= since) {
      match &event.happening {
        Happening::Siren(siren) if wanted(siren.session_id.as_deref()) => siren_count += 1,
        Happening::Usage(usage) if !usage.session_id.is_empty() && wanted(Some(&usage.session_id)) => {
          let index = match sessions.iter().position(|sums| sums.session_id == usage.session_id) {
            Some(index) => index,
            None => {
              sessions.push(SessionStats {
                session_id: usage.session_id.clone(),
                project_name: String::new(),
                harness_name: String::new(),
                model_name: String::new(),
                request_count: 0,
                new_token_count: 0,
                cached_token_count: 0,
                output_token_count: 0,
                cached_share: None,
                cold_count: 0,
                first_at: usage.at,
                last_at: usage.at,
              });
              sessions.len() - 1
            }
          };
          add_usage(&mut sessions[index], usage);
        }
        _ => {}
      }
    }
    for sums in &mut sessions {
      let total = sums.cached_token_count + sums.new_token_count;
      sums.cached_share = (total > 0).then(|| sums.cached_token_count as f64 / total as f64);
    }
    sessions.sort_by_key(|sums| std::cmp::Reverse(sums.last_at));
    Ok(Stats {
      sessions,
      siren_count,
      since,
    })
  }

  // ---- the event log

  /// Drop the event-log lines older than `retention_days`, by an atomic rewrite; 0 keeps everything.
  pub fn prune_events(&self) {
    let days = self.config().retention_days;
    let path = self.data.events();
    if days <= 0.0 || !path.exists() {
      return;
    }
    let horizon = now_millis() - (days * 86_400_000.0) as i64;
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
}

/// Which events a reader asked for: by kind and by session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
  kinds: Option<Vec<String>>,
  session_id: Option<String>,
}

impl Filter {
  /// The filter a query names; an unknown kind is refused.
  pub fn of(query: &EventQuery) -> Result<Filter, Refusal> {
    let kinds = match query.kinds.as_deref() {
      None => None,
      Some(kinds) => {
        let kinds: Vec<String> = kinds
          .split(',')
          .map(str::trim)
          .filter(|kind| !kind.is_empty())
          .map(Into::into)
          .collect();
        if let Some(unknown) = kinds.iter().find(|kind| !crate::wire::EVENT_KINDS.contains(&kind.as_str()))
        {
          return Err(Refusal::BadRequest(format!(
            "kinds names {unknown}, which is none of {}",
            crate::wire::EVENT_KINDS.join(", ")
          )));
        }
        Some(kinds)
      }
    };
    Ok(Filter {
      kinds,
      session_id: query.session_id.clone(),
    })
  }

  pub fn passes(&self, event: &Event) -> bool {
    let kind = event.happening.kind();
    self.kinds.as_ref().is_none_or(|kinds| kinds.iter().any(|want| want == kind))
      && self.session_id.as_deref().is_none_or(|want| event.happening.session_id() == Some(want))
  }
}

fn limit_of(limit: Option<u32>) -> Result<usize, Refusal> {
  match limit.unwrap_or(DEFAULT_LIMIT) {
    limit @ 1..=MOST_LIMIT => Ok(limit as usize),
    _ => Err(Refusal::BadRequest(format!("limit must be 1 to {MOST_LIMIT}"))),
  }
}

/// A cursor from before the window cannot be resumed from.
fn gone_check(state: &State, after: i64) -> Result<(), Refusal> {
  let start = state.window_start();
  if after < start - 1 {
    return Err(Refusal::Gone(format!(
      "the window starts at event {start}; read the page again without a cursor"
    )));
  }
  Ok(())
}

fn add_usage(sums: &mut SessionStats, usage: &Usage) {
  sums.project_name = usage.project_name.clone();
  sums.harness_name = usage.harness_name.clone();
  sums.model_name = usage.model_name.clone();
  if usage.new_token_count + usage.cached_token_count + usage.output_token_count > 0 {
    sums.request_count += 1;
  }
  sums.new_token_count += usage.new_token_count;
  sums.cached_token_count += usage.cached_token_count;
  sums.output_token_count += usage.output_token_count;
  if usage.kind == Kind::Cold {
    sums.cold_count += 1;
  }
  sums.first_at = sums.first_at.min(usage.at);
  sums.last_at = sums.last_at.max(usage.at);
}

/// The Siren an alert raised at `at`.
fn siren_of(alert: &Alert, at: i64, audible: bool) -> Siren {
  Siren {
    kind: alert.kind,
    at,
    session_id: (!alert.sid.is_empty()).then(|| alert.sid.clone()),
    project_name: alert.project.clone(),
    new_token_count: alert.new,
    cached_token_count: alert.read,
    request_count: alert.requests,
    note: alert.note.clone(),
    parts: alert
      .parts
      .iter()
      .map(|part| SirenPart {
        project_name: part.project.clone(),
        new_token_count: part.new,
        cached_token_count: part.read,
        request_count: part.req,
      })
      .collect(),
    audible,
  }
}

/// The newest `limit` events of the event log, and the highest number in it; a line of another shape is
/// skipped and does not cost the rest.
pub fn load_window(path: &Path, limit: usize) -> (VecDeque<Event>, i64) {
  let mut window = VecDeque::new();
  let mut highest = 0;
  let Ok(text) = fs::read(path) else { return (window, highest) };
  for raw in text.split(|&byte| byte == b'\n') {
    let Ok(event) = serde_json::from_slice::<Event>(raw) else { continue };
    highest = highest.max(event.id);
    if limit > 0 {
      if window.len() == limit {
        window.pop_front();
      }
      window.push_back(event);
    }
  }
  (window, highest)
}

/// A notable Usage's log line: its level, its message and its fields.
fn summary_line(usage: &Usage) -> (LogLevel, String, Value) {
  let level = if usage.level == Level::Red { LogLevel::Warn } else { LogLevel::Info };
  let message = format!(
    "{} {}/{} {} +{} read={} {} «{}»",
    usage.kind.as_str().to_uppercase(),
    usage.project_name,
    usage.session_id.chars().take(8).collect::<String>(),
    usage.model_label,
    thousands(usage.new_token_count),
    thousands(usage.cached_token_count),
    usage.note,
    usage.request_label
  );
  (
    level,
    message,
    json!({"lvl": usage.level, "cost": usage.cost_usd, "actor": usage.actor_name}),
  )
}

fn modified_millis(meta: &fs::Metadata) -> i64 {
  meta
    .modified()
    .ok()
    .and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok())
    .map_or(0, |since| since.as_millis() as i64)
}

/// Seconds as the burst note writes them: whole seconds without a fraction.
fn whole_number(seconds: f64) -> String {
  if seconds.fract() == 0.0 { format!("{}", seconds as i64) } else { seconds.to_string() }
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

/// Keep the event-log lines recorded at or after `horizon`; a line that is no event counts as oldest.
/// Rewritten only when something goes; the number removed and kept.
fn prune_file(path: &Path, horizon: i64) -> io::Result<(usize, usize)> {
  let text = fs::read(path)?;
  let mut kept = Vec::new();
  let mut removed = 0;
  let mut kept_count = 0;
  for raw in text.split(|&byte| byte == b'\n') {
    if raw.iter().all(u8::is_ascii_whitespace) {
      continue;
    }
    let at = serde_json::from_slice::<Event>(raw).map_or(i64::MIN, |event| event.at);
    if at >= horizon {
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
