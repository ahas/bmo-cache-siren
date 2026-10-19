//! The siren (docs/api.md, Voice and sound): a red alert outside its session's cooldown is logged and,
//! unless the overlay is muted, played and spoken.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::attribution::Kind;
use crate::clock::now_epoch;
use crate::config::Config;
use crate::data_dir::DataDir;
use crate::engine::{Alert, AlertHook};
use crate::event::thousands;
use crate::log::Log;

/// The player the siren sound is handed to.
const PLAYER: &str = "/usr/bin/afplay";

/// What tells the siren the time, in Unix seconds.
pub trait Clock: Send + Sync {
  fn now(&self) -> f64;
}

/// The host's own clock.
pub struct SystemClock;

impl Clock for SystemClock {
  fn now(&self) -> f64 {
    now_epoch()
  }
}

/// What starts the player and the voice command; neither's output is read.
pub trait Launcher: Send + Sync {
  fn launch(&self, argv: &[String]) -> io::Result<()>;
}

/// Starts each command as a child with no input or output, and reaps it when it ends.
pub struct ChildLauncher;

impl Launcher for ChildLauncher {
  fn launch(&self, argv: &[String]) -> io::Result<()> {
    let (program, args) = argv.split_first().ok_or_else(|| io::Error::other("empty command"))?;
    let mut child = Command::new(program)
      .args(args)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()?;
    std::thread::spawn(move || {
      let _ = child.wait();
    });
    Ok(())
  }
}

/// The keys the siren reads, all fixed at start.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
  pub sound: bool,
  pub siren_file: String,
  pub voice: bool,
  pub voice_cmd: Vec<String>,
  pub cooldown_s: f64,
  /// The generated sound, played when `siren_file` does not exist.
  pub siren_wav: PathBuf,
}

impl Settings {
  pub fn of(config: &Config, data: &DataDir) -> Settings {
    Settings {
      sound: config.sound,
      siren_file: config.siren_file.clone(),
      voice: config.voice,
      voice_cmd: config.voice_cmd.clone(),
      cooldown_s: config.siren_cooldown_s,
      siren_wav: data.siren_wav(),
    }
  }
}

pub struct Siren {
  settings: Settings,
  log: Arc<Log>,
  clock: Arc<dyn Clock>,
  launcher: Arc<dyn Launcher>,
  /// Session, or the kind of a sessionless alert → when it last sounded.
  last: Mutex<HashMap<String, f64>>,
}

impl Siren {
  pub fn new(
    settings: Settings,
    log: Arc<Log>,
    clock: Arc<dyn Clock>,
    launcher: Arc<dyn Launcher>,
  ) -> Siren {
    Siren {
      settings,
      log,
      clock,
      launcher,
      last: Mutex::new(HashMap::new()),
    }
  }

  /// A siren on the host's clock that starts real children.
  pub fn on_host(settings: Settings, log: Arc<Log>) -> Siren {
    Siren::new(settings, log, Arc::new(SystemClock), Arc::new(ChildLauncher))
  }

  /// Whether an alert sounds now: always when forced, else when its session's cooldown has passed. A
  /// sounding alert starts its cooldown again.
  fn due(&self, alert: &Alert) -> bool {
    let now = self.clock.now();
    let key = if alert.sid.is_empty() { alert.kind.as_str().to_string() } else { alert.sid.clone() };
    let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let cooling = last.get(&key).is_some_and(|&previous| now - previous < self.settings.cooldown_s);
    if !alert.force && cooling {
      return false;
    }
    last.insert(key, now);
    true
  }

  fn play(&self) {
    let file = &self.settings.siren_file;
    let sound = if !file.is_empty() && Path::new(file).exists() {
      PathBuf::from(file)
    } else {
      self.settings.siren_wav.clone()
    };
    let argv = [PLAYER.to_string(), sound.to_string_lossy().into_owned()];
    if let Err(why) = self.launcher.launch(&argv) {
      self.log.error("afplay failed", json!({"err": why.to_string(), "file": sound}));
    }
  }

  fn speak(&self, alert: &Alert) {
    let text = spoken(alert);
    let argv: Vec<String> =
      self.settings.voice_cmd.iter().map(|arg| arg.replace("{text}", &text)).collect();
    if let Err(why) = self.launcher.launch(&argv) {
      self.log.error("voice failed", json!({"err": why.to_string()}));
    }
  }
}

impl AlertHook for Siren {
  fn alert(&self, alert: &Alert, muted: bool) {
    if !self.due(alert) {
      return;
    }
    let message = format!(
      "SIREN {} {} +{} muted={muted}",
      alert.kind.as_str(),
      alert.project,
      thousands(alert.new)
    );
    self.log.warn(&message, json!({}));
    if muted {
      return;
    }
    if self.settings.sound {
      self.play();
    }
    if self.settings.voice && !self.settings.voice_cmd.is_empty() {
      self.speak(alert);
    }
  }
}

/// The English text an alert is spoken as.
pub fn spoken(alert: &Alert) -> String {
  let project = &alert.project;
  let new = alert.new;
  match alert.kind {
    Kind::Refusal => {
      format!(
        "Session {project}: a safeguard refusal switched the model. The next request re-prefills everything."
      )
    }
    Kind::Burst => format!("Burst: {new} new tokens within one minute. {project}."),
    Kind::Cold => {
      let cause = if alert.note.is_empty() { "unknown" } else { alert.note.as_str() };
      format!("Session {project} cold prefill, {new} new tokens. Cause {cause}.")
    }
    _ => format!("Session {project} grew by {new} new tokens."),
  }
}

/// Generate the siren sound when it is not there yet: 2.6 seconds of a tone sweeping between 620 and
/// 1100 Hz at 1.6 Hz, rising over 0.05 s and falling over the last 0.15 s, mono 16-bit at 22050 Hz.
pub fn ensure_siren_wav(path: &Path, log: &Log) {
  if path.exists() {
    return;
  }
  const RATE: u32 = 22_050;
  const SECONDS: f64 = 2.6;
  let spec = hound::WavSpec {
    channels: 1,
    sample_rate: RATE,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
  };
  let written = (|| -> Result<(), hound::Error> {
    let mut writer = hound::WavWriter::create(path, spec)?;
    let samples = (RATE as f64 * SECONDS) as u32;
    for index in 0..samples {
      let t = index as f64 / RATE as f64;
      let tone = 620.0 + 480.0 * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * 1.6 * t).cos());
      let envelope = (t / 0.05).min(1.0) * ((SECONDS - t) / 0.15).min(1.0);
      writer.write_sample((12_000.0 * envelope * (2.0 * std::f64::consts::PI * tone * t).sin()) as i16)?;
    }
    writer.finalize()
  })();
  match written {
    Ok(()) => log.info("siren.wav generated", json!({"path": path, "seconds": SECONDS})),
    Err(why) => log.error("siren.wav not generated", json!({"path": path, "err": why.to_string()})),
  }
}
