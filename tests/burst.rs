//! A burst line carries the real sums of its window and lists its owners largest first.

use cache_siren::{Burst, BurstPart, Recent, burst_summary};

fn recent(at: f64, new: u64, project: &str, read: u64) -> Recent {
  Recent {
    at,
    new,
    project: project.into(),
    read,
  }
}

#[test]
fn sums_new_and_read_and_orders_owners_by_new() {
  let window = [
    recent(100.0, 4000, "-home-user-repos-app", 575000),
    recent(101.0, 250000, "-home-user--local-share-agents-search", 8000000),
    recent(102.0, 252000, "-home-user--local-share-agents-search", 8400000),
    recent(103.0, 10, "", 0),
  ];
  let burst = burst_summary(&window);
  assert_eq!(burst.new, 506010);
  assert_eq!(burst.read, 16975000);
  assert_eq!(burst.req, 4);
  let owners: Vec<&str> = burst.parts.iter().map(|part| part.project.as_str()).collect();
  assert_eq!(owners, ["-home-user--local-share-agents-search", "-home-user-repos-app", "?"]);
  assert_eq!(
    burst.parts[0],
    BurstPart {
      project: "-home-user--local-share-agents-search".into(),
      new: 502000,
      read: 16400000,
      req: 2
    }
  );
}

#[test]
fn empty_window() {
  assert_eq!(
    burst_summary(&[]),
    Burst {
      new: 0,
      read: 0,
      req: 0,
      parts: Vec::new()
    }
  );
}
