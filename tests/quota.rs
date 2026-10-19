//! The header's quota is that of the account the source marks as supplying now.

use cache_siren::quota_from;
use serde_json::{Value, json};

fn slot(tier: &str, account: &str, five: u64, week: u64, active: Value) -> Value {
  json!({"five_hour_utilization": five, "five_hour_resets_at": 1000,
         "seven_day_utilization": week, "seven_day_resets_at": 2000,
         "tier": tier, "account": account, "provider": "claude-code", "active": active})
}

fn main_account_active() -> Value {
  json!({"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 2})
}

#[test]
fn reads_the_slot_marked_active_even_when_it_is_not_first() {
  let payload = json!({"usage": {
    "active": main_account_active(),
    "quota_slots": {
      "1": slot("Default account", "default", 96, 99, json!(false)),
      "2": slot("Main account", "main", 30, 58, json!(true)),
    },
  }});

  let quota = quota_from(&payload, "01:22");

  assert_eq!((quota.five_h, quota.seven_d, quota.tier), (json!(30), json!(58), json!("Main account")));
}

#[test]
fn reads_the_slot_the_active_account_names_when_no_slot_is_marked() {
  let payload = json!({"usage": {
    "active": main_account_active(),
    "quota_slots": {
      "1": slot("Default account", "default", 96, 99, Value::Null),
      "2": slot("Main account", "main", 30, 58, Value::Null),
    },
  }});

  let quota = quota_from(&payload, "01:22");

  assert_eq!((quota.five_h, quota.account, quota.accounts), (json!(30), json!("main"), json!(2)));
}

#[test]
fn a_source_with_one_bare_slot_reads_that_slot() {
  let payload =
    json!({"usage": {"quota_slots": {"1": {"five_hour_utilization": 12, "seven_day_utilization": 40}}}});

  let quota = quota_from(&payload, "01:22");

  assert_eq!(
    (quota.five_h, quota.seven_d, quota.account, quota.accounts),
    (json!(12), json!(40), Value::Null, Value::Null)
  );
}

#[test]
fn reads_the_first_slot_when_the_active_one_leads() {
  let payload = json!({"usage": {
    "active": main_account_active(),
    "quota_slots": {
      "1": slot("Main account", "main", 30, 58, json!(true)),
      "2": slot("Default account", "default", 96, 99, json!(false)),
    },
  }});

  let quota = quota_from(&payload, "01:22");

  assert_eq!((quota.five_h, quota.seven_d, quota.tier), (json!(30), json!(58), json!("Main account")));
}
