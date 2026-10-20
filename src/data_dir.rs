//! The data directory and the files in it (docs/api.md, Data directory). Only the siren writes it.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDir {
  root: PathBuf,
}

impl DataDir {
  pub fn new(root: impl Into<PathBuf>) -> DataDir {
    DataDir { root: root.into() }
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  /// The durable event log, one event per line.
  pub fn events(&self) -> PathBuf {
    self.root.join("events.jsonl")
  }

  /// The transcript offsets.
  pub fn state(&self) -> PathBuf {
    self.root.join("state.json")
  }

  /// The bearer token every API route takes, unless another file is named.
  pub fn token(&self) -> PathBuf {
    self.root.join("token")
  }

  pub fn log(&self) -> PathBuf {
    self.root.join("cache-siren.log")
  }

  /// The generated siren sound, played when no `siren_file` exists.
  pub fn siren_wav(&self) -> PathBuf {
    self.root.join("siren.wav")
  }
}
