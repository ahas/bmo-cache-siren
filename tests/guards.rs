//! What holds for the whole checkout: the manifest names only registry crates from its own list, and no
//! file holds Korean.

use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The crates the daemon is built from, each from the registry.
const DEPENDENCIES: [&str; 15] = [
  "axum",
  "chrono",
  "clap",
  "futures-util",
  "getrandom",
  "glob",
  "hound",
  "indexmap",
  "reqwest",
  "rmcp",
  "schemars",
  "serde",
  "serde_json",
  "tempfile",
  "tokio",
];
/// The crates only the tests use.
const DEV_DEPENDENCIES: [&str; 2] = ["regex", "toml"];

/// Folders holding version control, build output and installed packages, not files of the repository.
const UNGUARDED_DIRS: [&str; 5] = [".git", "target", "dist", ".build", "node_modules"];

/// Hangul Jamo, Compatibility Jamo, Jamo Extended-A, Syllables and Jamo Extended-B, by code point.
const HANGUL: [(u32, u32); 5] =
  [(0x1100, 0x11FF), (0x3130, 0x318F), (0xA960, 0xA97F), (0xAC00, 0xD7AF), (0xD7B0, 0xD7FF)];

fn manifest() -> toml::Table {
  fs::read_to_string(Path::new(ROOT).join("Cargo.toml")).unwrap().parse().unwrap()
}

/// The crate names of one dependency table, sorted; each must come from the registry, never from a path,
/// a git repository or another bmo package.
fn registry_crates(manifest: &toml::Table, table: &str) -> Vec<String> {
  let Some(crates) = manifest.get(table).and_then(toml::Value::as_table) else {
    return Vec::new();
  };
  for (name, spec) in crates {
    let spec = spec.as_table();
    assert!(!name.starts_with("bmo-"), "{table}: {name} is a bmo package");
    assert!(
      spec.is_none_or(|spec| !spec.contains_key("path")),
      "{table}: {name} is taken from a path"
    );
    assert!(spec.is_none_or(|spec| !spec.contains_key("git")), "{table}: {name} is taken from git");
  }
  let mut names: Vec<String> = crates.keys().cloned().collect();
  names.sort();
  names
}

#[test]
fn the_manifest_names_only_its_own_registry_crates() {
  let manifest = manifest();
  assert_eq!(registry_crates(&manifest, "dependencies"), DEPENDENCIES);
  assert_eq!(registry_crates(&manifest, "dev-dependencies"), DEV_DEPENDENCIES);
  assert!(registry_crates(&manifest, "build-dependencies").is_empty());
  assert!(manifest.get("target").is_none(), "no target-specific dependencies");
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
  for entry in fs::read_dir(dir).unwrap() {
    let path = entry.unwrap().path();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    if path.is_dir() {
      if !UNGUARDED_DIRS.contains(&name.as_str()) {
        walk(&path, found);
      }
    } else {
      found.push(path);
    }
  }
}

fn is_hangul(c: char) -> bool {
  HANGUL.iter().any(|&(low, high)| (low..=high).contains(&(c as u32)))
}

#[test]
fn no_hangul_in_any_file_of_the_checkout() {
  let mut files = Vec::new();
  walk(Path::new(ROOT), &mut files);
  let mut found = Vec::new();
  for path in files {
    let Ok(text) = fs::read_to_string(&path) else { continue };
    for (number, line) in text.lines().enumerate() {
      if line.chars().any(is_hangul) {
        found.push(format!("{}:{}", path.strip_prefix(ROOT).unwrap().display(), number + 1));
      }
    }
  }
  assert_eq!(found, Vec::<String>::new());
}
