//! The siren's cooldown, arming, player and voice, on a clock the test moves.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cache_siren::paths::home;
use cache_siren::siren::{Clock, Launcher, Settings, Siren, ensure_siren_wav, host_player, player_argv};
use cache_siren::{Alert, AlertHook, Config, DataDir, Kind, Log};

#[derive(Default)]
struct Hand(Mutex<f64>);

impl Hand {
  fn set(&self, now: f64) {
    *self.0.lock().unwrap() = now;
  }
}

impl Clock for Hand {
  fn now(&self) -> f64 {
    *self.0.lock().unwrap()
  }
}

#[derive(Default)]
struct Launched(Mutex<Vec<Vec<String>>>);

impl Launched {
  fn take(&self) -> Vec<Vec<String>> {
    std::mem::take(&mut self.0.lock().unwrap())
  }
}

impl Launcher for Launched {
  fn launch(&self, argv: &[String]) -> io::Result<()> {
    self.0.lock().unwrap().push(argv.to_vec());
    Ok(())
  }
}

fn cold(sid: &str, force: bool) -> Alert {
  Alert {
    kind: Kind::Cold,
    project: "app".into(),
    sid: sid.into(),
    new: 150_000,
    read: 0,
    requests: 1,
    note: String::new(),
    parts: Vec::new(),
    force,
  }
}

struct Fixture {
  dir: tempfile::TempDir,
  clock: Arc<Hand>,
  launched: Arc<Launched>,
  siren: Siren,
}

fn fixture() -> Fixture {
  let dir = tempfile::tempdir().unwrap();
  let settings = Settings {
    sound: true,
    siren_file: dir.path().join("warning.mp3"),
    player: vec!["play".into(), "-q".into(), "{file}".into()],
    voice: true,
    voice_cmd: vec!["say".into(), "-v".into(), "{text}".into()],
    cooldown_s: 20.0,
    siren_wav: dir.path().join("siren.wav"),
  };
  let log = Arc::new(Log::new(dir.path().join("cache-siren.log"), true));
  let clock = Arc::new(Hand::default());
  let launched = Arc::new(Launched::default());
  let siren = Siren::new(settings, log, clock.clone(), launched.clone());
  Fixture {
    dir,
    clock,
    launched,
    siren,
  }
}

#[test]
fn a_session_sounds_once_per_cooldown_and_a_forced_alert_always() {
  let fixture = fixture();
  let wav = fixture.dir.path().join("siren.wav").to_string_lossy().into_owned();
  fixture.clock.set(1000.0);

  assert_eq!(fixture.siren.alert(&cold("s1", false), true), Some(true));
  let launched = fixture.launched.take();
  assert_eq!(
    launched[0],
    ["play".to_string(), "-q".into(), wav],
    "no siren file, so the generated sound"
  );
  assert_eq!(
    launched[1],
    ["say", "-v", "Session app cold prefill, 150000 new tokens. Cause unknown."]
  );

  fixture.clock.set(1019.0);
  assert_eq!(fixture.siren.alert(&cold("s1", false), true), None, "inside the cooldown");
  assert!(fixture.launched.take().is_empty());
  fixture.siren.alert(&cold("s2", false), true);
  assert_eq!(fixture.launched.take().len(), 2, "another session has its own cooldown");
  fixture.siren.alert(&cold("s1", true), true);
  assert_eq!(fixture.launched.take().len(), 2, "a forced alert ignores the cooldown");

  fixture.clock.set(1040.0);
  assert_eq!(fixture.siren.alert(&cold("s1", false), false), Some(false), "quiet: raised, not played");
  assert!(fixture.launched.take().is_empty());
  fixture.clock.set(1050.0);
  assert_eq!(
    fixture.siren.alert(&cold("s1", false), true),
    None,
    "a quiet alert still starts the cooldown"
  );
}

#[test]
fn a_host_with_no_player_set_takes_its_own() {
  let dir = tempfile::tempdir().unwrap();
  assert_eq!(host_player(true, None), ["afplay", "{file}"]);
  std::fs::write(dir.path().join("paplay"), b"").unwrap();
  std::fs::write(dir.path().join("aplay"), b"").unwrap();
  let path = std::env::join_paths([PathBuf::from("/nowhere"), dir.path().to_path_buf()]).unwrap();
  assert_eq!(host_player(false, Some(path)), ["paplay", "{file}"], "the first of the list found");
  assert_eq!(
    host_player(false, Some(OsString::new()))[0],
    "pw-play",
    "none found: its failure is logged"
  );

  assert_eq!(
    player_argv(&["paplay".into()], "/s.wav"),
    ["paplay", "/s.wav"],
    "appended where unmarked"
  );
}

#[test]
fn a_siren_file_under_the_home_folder_is_read_through_its_tilde() {
  let dir = tempfile::tempdir().unwrap();
  let config = Config {
    siren_file: "~/sounds/warning.mp3".into(),
    siren_player: vec!["play".into()],
    ..Config::defaults(dir.path())
  };
  let settings = Settings::of(&config, &DataDir::new(dir.path()));
  assert_eq!(settings.siren_file, home().join("sounds/warning.mp3"));
  assert_eq!(settings.player, ["play"], "a configured player is taken as given");
}

#[test]
fn the_siren_file_is_played_when_it_exists_and_the_wav_is_generated_once() {
  let fixture = fixture();
  let file: PathBuf = fixture.dir.path().join("warning.mp3");
  std::fs::write(&file, b"mp3").unwrap();

  fixture.siren.alert(&cold("s1", false), true);
  assert_eq!(fixture.launched.take()[0][2], file.to_string_lossy());

  let wav = fixture.dir.path().join("siren.wav");
  let log = Log::new(fixture.dir.path().join("cache-siren.log"), true);
  ensure_siren_wav(&wav, &log);
  let reader = hound::WavReader::open(&wav).unwrap();
  let spec = reader.spec();
  assert_eq!((spec.channels, spec.sample_rate, spec.bits_per_sample), (1, 22_050, 16));
  assert_eq!(reader.duration(), 57_330);
}
