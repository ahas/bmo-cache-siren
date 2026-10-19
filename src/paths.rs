//! Where the siren looks by default: the home folder, the CLI's transcript root and the data directory.

use std::path::{Path, PathBuf};

/// The home folder; the current directory where the system names none.
pub fn home() -> PathBuf {
  std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The Claude Code CLI's own transcript root, used when no `projects_globs` root matches.
pub fn default_projects_dir() -> PathBuf {
  home().join(".claude").join("projects")
}

/// The data directory the siren writes: logs, the event log, offsets, the overlay state, the sound.
pub fn default_data_dir() -> PathBuf {
  home().join(".claude").join("cache-siren")
}

/// A path with a leading `~` or `~/` read as the home folder; any other path as written.
///
/// Written here because the standard library has no home expansion and `glob` matches only what it is
/// given; only the current user's `~` is read, never `~name`.
pub fn expand_tilde(path: &str) -> PathBuf {
  match path.strip_prefix('~') {
    Some("") => home(),
    Some(rest) if rest.starts_with('/') => home().join(rest.trim_start_matches('/')),
    _ => Path::new(path).to_path_buf(),
  }
}
