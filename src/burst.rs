//! The burst window gathered into one line: the real sums of its requests, its owners largest first.

use indexmap::IndexMap;
use serde::Serialize;

/// One request inside the burst window.
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
  /// Unix seconds of the request.
  pub at: f64,
  pub new: u64,
  pub project: String,
  pub read: u64,
}

/// One project's share of the window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BurstPart {
  pub project: String,
  pub new: u64,
  pub read: u64,
  pub req: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Burst {
  pub new: u64,
  pub read: u64,
  pub req: u64,
  /// Largest new tokens first, so the first project owns the burst; a request with no project is `?`.
  pub parts: Vec<BurstPart>,
}

/// Sum the window's requests by project.
pub fn burst_summary<'a>(recent: impl IntoIterator<Item = &'a Recent>) -> Burst {
  let mut by_project: IndexMap<&str, BurstPart> = IndexMap::new();
  for request in recent {
    let name = if request.project.is_empty() { "?" } else { request.project.as_str() };
    let part = by_project.entry(name).or_insert_with(|| BurstPart {
      project: name.into(),
      new: 0,
      read: 0,
      req: 0,
    });
    part.new += request.new;
    part.read += request.read;
    part.req += 1;
  }
  let mut parts: Vec<BurstPart> = by_project.into_values().collect();
  parts.sort_by_key(|part| std::cmp::Reverse(part.new));
  Burst {
    new: parts.iter().map(|part| part.new).sum(),
    read: parts.iter().map(|part| part.read).sum(),
    req: parts.iter().map(|part| part.req).sum(),
    parts,
  }
}
