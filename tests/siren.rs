//! The siren's cooldown, mute, player and voice, on a clock the test moves.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cache_siren::siren::{Clock, Launcher, Settings, Siren, ensure_siren_wav};
use cache_siren::{Alert, AlertHook, Kind, Log};

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
    note: String::new(),
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
    siren_file: dir.path().join("warning.mp3").to_string_lossy().into_owned(),
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

  fixture.siren.alert(&cold("s1", false), false);
  let launched = fixture.launched.take();
  assert_eq!(
    launched[0],
    ["/usr/bin/afplay".to_string(), wav],
    "no siren file, so the generated sound"
  );
  assert_eq!(
    launched[1],
    ["say", "-v", "Session app cold prefill, 150000 new tokens. Cause unknown."]
  );

  fixture.clock.set(1019.0);
  fixture.siren.alert(&cold("s1", false), false);
  assert!(fixture.launched.take().is_empty(), "inside the cooldown");
  fixture.siren.alert(&cold("s2", false), false);
  assert_eq!(fixture.launched.take().len(), 2, "another session has its own cooldown");
  fixture.siren.alert(&cold("s1", true), false);
  assert_eq!(fixture.launched.take().len(), 2, "a forced alert ignores the cooldown");

  fixture.clock.set(1040.0);
  fixture.siren.alert(&cold("s1", false), true);
  assert!(fixture.launched.take().is_empty(), "muted: logged, not played");
  fixture.clock.set(1050.0);
  fixture.siren.alert(&cold("s1", false), false);
  assert!(fixture.launched.take().is_empty(), "a muted alert still starts the cooldown");
}

#[test]
fn the_siren_file_is_played_when_it_exists_and_the_wav_is_generated_once() {
  let fixture = fixture();
  let file: PathBuf = fixture.dir.path().join("warning.mp3");
  std::fs::write(&file, b"mp3").unwrap();

  fixture.siren.alert(&cold("s1", false), false);
  assert_eq!(fixture.launched.take()[0][1], file.to_string_lossy());

  let wav = fixture.dir.path().join("siren.wav");
  let log = Log::new(fixture.dir.path().join("cache-siren.log"), true);
  ensure_siren_wav(&wav, &log);
  let reader = hound::WavReader::open(&wav).unwrap();
  let spec = reader.spec();
  assert_eq!((spec.channels, spec.sample_rate, spec.bits_per_sample), (1, 22_050, 16));
  assert_eq!(reader.duration(), 57_330);
}
