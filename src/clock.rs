//! Host-local time: every `ts_kst` and `date_kst` the siren writes is in the host's own zone.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// Now, in the host's zone.
pub fn now_local() -> DateTime<Local> {
  Local::now()
}

/// A transcript `timestamp` (ISO 8601, usually UTC with `Z`) in the host's zone; a timestamp with no
/// offset is read as host-local. None when it cannot be read.
pub fn ts_local(iso: &str) -> Option<DateTime<Local>> {
  if let Ok(at) = DateTime::parse_from_rfc3339(iso) {
    return Some(at.with_timezone(&Local));
  }
  let naive = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
  Local.from_local_datetime(&naive).earliest()
}
