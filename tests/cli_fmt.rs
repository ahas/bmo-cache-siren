//! What the command-line verbs print from the daemon's answers: the status line, the tail table and the
//! toggled overlay state.

use cache_siren::cli::{NOT_RESPONDING, status_line, tail_lines, toggled};
use serde_json::json;

#[test]
fn status_names_uptime_files_events_the_newest_row_and_the_quota() {
  let health = json!({"ok": true, "uptime_s": 42, "files": 3, "events": 17, "last_event": "13:52:37",
                      "quota": {"five_h": 30}});
  assert_eq!(
    status_line(&health),
    "✅ cache-siren up 42s · files 3 · events 17 · last 13:52:37 · 5h 30%"
  );

  let fresh = json!({"ok": true, "uptime_s": 0, "files": 0, "events": 0, "last_event": null, "quota": {}});
  assert_eq!(status_line(&fresh), "✅ cache-siren up 0s · files 0 · events 0 · last None · 5h None%");
  assert_eq!(NOT_RESPONDING, "❌ cache-siren daemon not responding");
}

#[test]
fn tail_pads_cuts_and_groups_each_row() {
  let feed = json!({"events": [
    {"ts_kst": "13:52:37", "project": "a-very-long-project-name", "session": "fcf2219e", "m": "Opus 5",
     "kind": "cold", "new": 150000, "read": 0, "note": "prefix_changed_by_a_lot", "req": "⌨ check the build"},
    {"ts_kst": "13:53:01", "kind": "burst", "new": 506010, "read": 16975000},
  ], "seq": 2});

  assert_eq!(
    tail_lines(&feed),
    [
      "13:52:37 a-very-long- fcf2219e Opus 5    cold    + 150,000 read=        0 prefix_changed ⌨ check the build",
      "13:53:01                                 burst   + 506,010 read=16,975,000                ",
    ]
  );
}

#[test]
fn toggle_flips_the_overlay_and_reads_an_unsaid_one_as_visible() {
  assert!(!toggled(&json!({"visible": true})));
  assert!(toggled(&json!({"visible": false})));
  assert!(!toggled(&json!({})));
}
