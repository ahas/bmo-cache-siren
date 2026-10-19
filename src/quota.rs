//! The quota source's answer read down to the one slot the header shows (docs/api.md, Quota source).

use serde::Serialize;
use serde_json::{Map, Value};

/// The slot read from one poll; each value as the source gave it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotaReading {
  pub five_h: Value,
  pub five_h_resets: Value,
  pub seven_d: Value,
  pub seven_d_resets: Value,
  pub tier: Value,
  pub account: Value,
  pub provider: Value,
  pub accounts: Value,
  /// Local `HH:MM` of the poll.
  pub at: String,
}

/// Read the slot the source marks as supplying now, in this order: the slot whose `active` is exactly
/// true; the slot whose `account` is `usage.active.account`; slot `"1"`; none. A slot lacking the
/// utilisation fields falls back to `usage.real_5h_pct` and `usage.real_week_pct`.
pub fn quota_from(payload: &Value, at: &str) -> QuotaReading {
  let usage = object(payload.get("usage"));
  let active = object(usage.and_then(|usage| usage.get("active")));
  let slots = object(usage.and_then(|usage| usage.get("quota_slots")));

  let mut keys: Vec<&String> = slots.map(|slots| slots.keys().collect()).unwrap_or_default();
  keys.sort_by_key(|key| key.parse::<u64>().unwrap_or(0));
  let ordered: Vec<&Map<String, Value>> = keys
    .into_iter()
    .filter_map(|key| object(slots.and_then(|slots| slots.get(key))))
    .filter(|s| !s.is_empty())
    .collect();

  let marked = ordered.iter().copied().find(|slot| slot.get("active") == Some(&Value::Bool(true)));
  let named = || {
    let account = active?.get("account")?;
    ordered
      .iter()
      .copied()
      .find(|slot| slot.get("account").unwrap_or(&Value::Null) == account)
  };
  let first = || object(slots.and_then(|slots| slots.get("1"))).filter(|slot| !slot.is_empty());
  let slot = marked.or_else(named).or_else(first);

  let from_slot = |key: &str| slot.and_then(|slot| slot.get(key)).cloned();
  let from_usage = |key: &str| usage.and_then(|usage| usage.get(key)).cloned().unwrap_or(Value::Null);
  let from_active = |key: &str| active.and_then(|active| active.get(key)).cloned();
  QuotaReading {
    five_h: from_slot("five_hour_utilization").unwrap_or_else(|| from_usage("real_5h_pct")),
    five_h_resets: from_slot("five_hour_resets_at").unwrap_or(Value::Null),
    seven_d: from_slot("seven_day_utilization").unwrap_or_else(|| from_usage("real_week_pct")),
    seven_d_resets: from_slot("seven_day_resets_at").unwrap_or(Value::Null),
    tier: from_slot("tier")
      .or_else(|| from_active("name"))
      .unwrap_or_else(|| Value::String(String::new())),
    account: from_slot("account").or_else(|| from_active("account")).unwrap_or(Value::Null),
    provider: from_slot("provider").or_else(|| from_active("provider")).unwrap_or(Value::Null),
    accounts: from_active("accounts").unwrap_or(Value::Null),
    at: at.to_string(),
  }
}

/// A value read as an object; none for anything else, as a missing one is.
fn object(value: Option<&Value>) -> Option<&Map<String, Value>> {
  value.and_then(Value::as_object)
}
