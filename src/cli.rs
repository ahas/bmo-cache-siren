//! The command's verbs against a running daemon (docs/api.md, The command): each calls the API through the
//! client and prints a line of its answer, or with `--json` the answer exactly as the API gave it.

use std::path::Path;
use std::process::{Command, ExitCode};

use chrono::{Local, TimeZone};
use serde::Serialize;

use crate::client::{ClientFailure, SirenClient};
use crate::event::thousands;
use crate::wire::{Event, EventPage, Happening, Health, Settings, Stats};

/// The `status` line, from the health, the settings and the stats.
pub fn status_line(health: &Health, settings: &Settings, stats: &Stats) -> String {
  let ready = match (&health.ready, &health.reason) {
    (true, _) => "ready".to_string(),
    (false, reason) => format!("not ready: {}", reason.as_deref().unwrap_or("no reason given")),
  };
  format!(
    "bmo-cache-siren {} {ready} · {} · threshold {} · {} sessions · {} sirens since {}",
    health.version,
    if settings.armed { "armed" } else { "quiet" },
    thousands(settings.threshold),
    stats.sessions.len(),
    stats.siren_count,
    clock(stats.since),
  )
}

/// A time as the host's own `HH:MM:SS`.
fn clock(at: i64) -> String {
  Local
    .timestamp_millis_opt(at)
    .single()
    .map_or_else(|| at.to_string(), |at| at.format("%H:%M:%S").to_string())
}

fn first(text: &str, chars: usize) -> String {
  text.chars().take(chars).collect()
}

/// One event as a `tail` line.
pub fn event_line(event: &Event) -> String {
  match &event.happening {
    Happening::Usage(usage) => format!(
      "{:8} {:12} {:8} {:9} {:7} +{:>8} read={:>9} {:14} {}",
      clock(usage.at),
      first(&usage.project_name, 12),
      first(&usage.session_id, 8),
      usage.model_label,
      usage.kind.as_str(),
      thousands(usage.new_token_count),
      thousands(usage.cached_token_count),
      first(&usage.note, 14),
      first(&usage.request_label, 60)
    ),
    Happening::Siren(siren) => format!(
      "{:8} SIREN {} {} +{} {}{}",
      clock(siren.at),
      siren.kind.as_str(),
      siren.project_name,
      thousands(siren.new_token_count),
      siren.note,
      if siren.audible { "" } else { " (quiet)" }
    ),
    Happening::Settings(settings) => format!(
      "{:8} settings {} · ttl {} ms · threshold {}",
      clock(event.at),
      if settings.armed { "armed" } else { "quiet" },
      settings.ttl_ms,
      thousands(settings.threshold)
    ),
  }
}

/// The `tail` lines, one per event of a page.
pub fn tail_lines(page: &EventPage) -> Vec<String> {
  page.items.iter().map(event_line).collect()
}

/// The `stats` lines, one per session.
pub fn stats_lines(stats: &Stats) -> Vec<String> {
  let mut lines = vec![format!("since {} · {} sirens", clock(stats.since), stats.siren_count)];
  for session in &stats.sessions {
    let share = session.cached_share.map_or("-".to_string(), |share| format!("{:.0}%", share * 100.0));
    lines.push(format!(
      "{:8} {:12} {:12} req {:>4} new {:>11} cached {:>11} ({share}) cold {}",
      first(&session.session_id, 8),
      first(&session.project_name, 12),
      first(&session.model_name, 12),
      session.request_count,
      thousands(session.new_token_count),
      thousands(session.cached_token_count),
      session.cold_count
    ));
  }
  lines
}

/// The `settings` line.
pub fn settings_line(settings: &Settings) -> String {
  format!(
    "{} · ttl {} ms · threshold {}",
    if settings.armed { "armed" } else { "quiet" },
    settings.ttl_ms,
    thousands(settings.threshold)
  )
}

/// Print an answer: as the API gave it with `json`, else as `lines` write it. A failure is printed to
/// standard error and ends the verb with failure.
pub fn print<T: Serialize>(
  answer: Result<T, ClientFailure>,
  json: bool,
  lines: impl FnOnce(&T) -> Vec<String>,
) -> ExitCode {
  match answer {
    Ok(answer) => {
      if json {
        println!("{}", serde_json::to_string_pretty(&answer).unwrap_or_default());
      } else {
        for line in lines(&answer) {
          println!("{line}");
        }
      }
      ExitCode::SUCCESS
    }
    Err(failure) => {
      eprintln!("bmo-cache-siren: {failure}");
      ExitCode::FAILURE
    }
  }
}

/// `status`: the health with `json`, else the health, settings and stats on one line.
pub fn status(client: &SirenClient, json: bool) -> ExitCode {
  if json {
    return print(client.health(), true, |_| Vec::new());
  }
  let answer = client.health().and_then(|health| {
    let settings = client.settings()?;
    let stats = client.stats(&Default::default())?;
    Ok(status_line(&health, &settings, &stats))
  });
  print(answer, false, |line| vec![line.clone()])
}

/// The live page's address with the token in its fragment.
pub fn page_address(address: &str, token: &str) -> String {
  format!("{}/#access_token={token}", address.trim_end_matches('/'))
}

/// Restart the daemon under the host's service manager: the daemon and the overlay app under launchd
/// on macOS, the user unit under systemd elsewhere.
pub fn restart() -> ExitCode {
  let started = if cfg!(target_os = "macos") {
    let uid = Command::new("id")
      .arg("-u")
      .output()
      .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let Ok(uid) = uid else { return ExitCode::FAILURE };
    ["local.cache-siren", "local.cache-siren.app"].iter().all(|label| {
      Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/{label}")])
        .status()
        .is_ok_and(|status| status.success())
    })
  } else {
    Command::new("systemctl")
      .args(["--user", "restart", "cache-siren"])
      .status()
      .is_ok_and(|status| status.success())
  };
  if started {
    println!("restarted");
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

/// Print the last `lines` lines of the daemon's log.
pub fn log_tail(path: &Path, lines: usize) -> ExitCode {
  let Ok(text) = std::fs::read_to_string(path) else {
    eprintln!("bmo-cache-siren: cannot read {}", path.display());
    return ExitCode::FAILURE;
  };
  let all: Vec<&str> = text.lines().collect();
  for line in &all[all.len().saturating_sub(lines)..] {
    println!("{line}");
  }
  ExitCode::SUCCESS
}
