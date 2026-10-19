//! The structured log, `cache-siren.log` in the data directory: one line per entry,
//! `[<local time>] [cache-siren] [<LEVEL>] <message> <fields as JSON>`, echoed to standard output unless
//! quiet.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::clock::now_local;

/// Past this size the log keeps only its tail, so it never grows without bound.
const TRIM_PAST_BYTES: u64 = 8_000_000;
/// How much of the tail a trim keeps; the lines are structured, so losing the head is harmless.
const KEEP_BYTES: i64 = 4_000_000;
const TRIMMED_MARK: &[u8] = b"[log truncated to last ~4MB]\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
  Info,
  Warn,
  Error,
}

impl Level {
  pub fn as_str(self) -> &'static str {
    match self {
      Level::Info => "INFO",
      Level::Warn => "WARN",
      Level::Error => "ERROR",
    }
  }
}

/// One line of the log, without its newline. An empty object or a null adds no fields.
pub fn format_line(at: DateTime<Local>, level: Level, message: &str, fields: &Value) -> String {
  let mut line =
    format!("[{}] [cache-siren] [{}] {message}", at.format("%Y-%m-%dT%H:%M:%S"), level.as_str());
  let empty = match fields {
    Value::Null => true,
    Value::Object(map) => map.is_empty(),
    _ => false,
  };
  if !empty {
    line.push(' ');
    line.push_str(&fields.to_string());
  }
  line
}

/// The log file and whether lines are also echoed. A line that cannot be written is dropped: the log
/// never stops the daemon.
pub struct Log {
  file: PathBuf,
  quiet: bool,
  lock: Mutex<()>,
}

impl Log {
  pub fn new(file: impl Into<PathBuf>, quiet: bool) -> Log {
    Log {
      file: file.into(),
      quiet,
      lock: Mutex::new(()),
    }
  }

  pub fn file(&self) -> &Path {
    &self.file
  }

  pub fn line(&self, level: Level, message: &str, fields: Value) {
    let line = format_line(now_local(), level, message, &fields);
    {
      let _held = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      let _ = trim(&self.file);
      let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&self.file)
        .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()));
    }
    if !self.quiet {
      println!("{line}");
    }
  }

  pub fn info(&self, message: &str, fields: Value) {
    self.line(Level::Info, message, fields);
  }

  pub fn warn(&self, message: &str, fields: Value) {
    self.line(Level::Warn, message, fields);
  }

  pub fn error(&self, message: &str, fields: Value) {
    self.line(Level::Error, message, fields);
  }
}

/// Cut the file to the lines inside its last `KEEP_BYTES` once it passes `TRIM_PAST_BYTES`.
fn trim(path: &Path) -> std::io::Result<()> {
  if fs::metadata(path)?.len() <= TRIM_PAST_BYTES {
    return Ok(());
  }
  let mut file = File::open(path)?;
  file.seek(SeekFrom::End(-KEEP_BYTES))?;
  let mut tail = Vec::new();
  file.read_to_end(&mut tail)?;
  let cut = tail.iter().position(|&byte| byte == b'\n').map_or(0, |at| at + 1);
  let mut kept = TRIMMED_MARK.to_vec();
  kept.extend_from_slice(&tail[cut..]);
  fs::write(path, kept)
}
