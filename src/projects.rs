//! Transcript roots and what a transcript's path says: its project, and whether it is a subagent's.
//!
//! With several logins the CLI writes transcripts under each account's own config directory, so there is
//! not one root but several; `projects_globs` lists them as globs, so a newly added account is picked up
//! as it is.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::config::Config;
use crate::paths::{default_projects_dir, expand_tilde};

/// The transcript roots: every directory the globs name, sorted and without repeats; the CLI's own root
/// when none matches.
pub fn proj_dirs(config: &Config) -> Vec<PathBuf> {
  let defaults = [default_projects_dir().to_string_lossy().into_owned()];
  let patterns = if config.projects_globs.is_empty() {
    &defaults[..]
  } else {
    &config.projects_globs[..]
  };
  let mut seen = HashSet::new();
  let mut roots = Vec::new();
  for pattern in patterns {
    let Ok(found) = glob::glob(&expand_tilde(pattern).to_string_lossy()) else { continue };
    let mut found: Vec<PathBuf> = found.filter_map(Result::ok).filter(|path| path.is_dir()).collect();
    found.sort();
    for root in found {
      if seen.insert(root.clone()) {
        roots.push(root);
      }
    }
  }
  if roots.is_empty() {
    roots.push(default_projects_dir());
  }
  roots
}

/// The root a transcript sits under; the CLI's own root when none holds it.
pub fn proj_root_of(path: &Path, config: &Config) -> PathBuf {
  proj_dirs(config)
    .into_iter()
    .find(|root| path.starts_with(root))
    .unwrap_or_else(default_projects_dir)
}

/// What a transcript's path says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
  /// The encoded project folder, less the first configured prefix it starts with and its leading
  /// dashes.
  pub project: String,
  /// Whether it is a subagent's transcript, `<project>/<session>/subagents/<agent>.jsonl`.
  pub is_agent: bool,
  /// The parent session of a subagent's transcript.
  pub parent: Option<String>,
}

/// The project and the subagent standing of the transcript at `path`.
pub fn project_of(path: &Path, config: &Config) -> Owner {
  let root = proj_root_of(path, config);
  let relative = path.strip_prefix(&root).unwrap_or(path);
  let parts: Vec<String> = relative
    .components()
    .filter_map(|part| match part {
      Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
      _ => None,
    })
    .collect();
  let mut project = parts.first().cloned().unwrap_or_default();
  if let Some(prefix) = config.project_prefixes.iter().find(|prefix| project.starts_with(prefix.as_str())) {
    project = project[prefix.len()..].to_string();
  }
  let project = project.trim_start_matches('-').to_string();
  let is_agent = parts.len() >= 3 && parts[parts.len() - 2] == "subagents";
  let parent = if is_agent { parts.get(1).cloned() } else { None };
  Owner {
    project,
    is_agent,
    parent,
  }
}
