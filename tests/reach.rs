//! A verb's address (docs/api.md, the command): `--address`, else the loopback port of the config file the
//! daemon reads, the default port only where no config file stands, and never a config file written.

use std::path::Path;
use std::process::Command;

/// The live page's address `page` prints over the data directory `data`, with `arguments` added.
fn page(data: &Path, arguments: &[&str]) -> String {
  let ran = Command::new(env!("CARGO_BIN_EXE_bmo-cache-siren"))
    .arg("page")
    .args(["--token", "a-token", "--data-dir"])
    .arg(data)
    .args(arguments)
    .env_remove("CACHE_SIREN_ADDRESS")
    .env_remove("CACHE_SIREN_CONFIG")
    .output()
    .expect("the binary runs");
  assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
  String::from_utf8(ran.stdout).expect("the address is text")
}

#[test]
fn the_address_is_the_port_the_config_in_the_data_directory_names() {
  let data = tempfile::tempdir().unwrap();
  std::fs::write(data.path().join("config.json"), r#"{ "port": 4321 }"#).unwrap();

  assert!(
    page(data.path(), &[]).starts_with("http://127.0.0.1:4321"),
    "{}",
    page(data.path(), &[])
  );
  let named = page(data.path(), &["--address", "http://127.0.0.1:5000"]);
  assert!(named.starts_with("http://127.0.0.1:5000"), "{named}");
  assert_eq!(std::fs::read_to_string(data.path().join("config.json")).unwrap(), r#"{ "port": 4321 }"#);
}

#[test]
fn a_named_config_stands_over_the_data_directorys_and_none_standing_gives_the_default_port() {
  let data = tempfile::tempdir().unwrap();
  std::fs::write(data.path().join("config.json"), r#"{ "port": 4321 }"#).unwrap();
  let other = data.path().join("other.json");
  std::fs::write(&other, r#"{ "port": 4400, "armed": false }"#).unwrap();

  let named = page(data.path(), &["--config", other.to_str().unwrap()]);
  assert!(named.starts_with("http://127.0.0.1:4400"), "{named}");
  let missing = data.path().join("missing.json");
  let none = page(data.path(), &["--config", missing.to_str().unwrap()]);
  assert!(none.starts_with("http://127.0.0.1:9022"), "{none}");
  assert!(!missing.exists(), "a verb wrote the config file");
}

#[test]
fn a_config_that_does_not_read_gives_the_default_port_the_daemon_then_runs_on() {
  let data = tempfile::tempdir().unwrap();
  std::fs::write(data.path().join("config.json"), "{ not json").unwrap();

  assert!(
    page(data.path(), &[]).starts_with("http://127.0.0.1:9022"),
    "{}",
    page(data.path(), &[])
  );
}
