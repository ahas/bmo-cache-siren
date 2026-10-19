//! The overlay control state (docs/api.md, `/ctl`), kept in the data directory across restarts.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ctl {
  pub visible: bool,
  /// Silences the siren and the voice; the alert is still logged.
  pub muted: bool,
  pub opacity: f64,
  pub show_local: bool,
  pub page: String,
}

impl Default for Ctl {
  fn default() -> Ctl {
    Ctl {
      visible: true,
      muted: false,
      opacity: 0.94,
      show_local: false,
      page: "/".into(),
    }
  }
}

impl Ctl {
  /// Set each of the five keys `body` gives with a value of its type; every other key is ignored.
  pub fn merge(&mut self, body: &Map<String, Value>) {
    let flag = |key: &str| body.get(key).and_then(Value::as_bool);
    if let Some(visible) = flag("visible") {
      self.visible = visible;
    }
    if let Some(muted) = flag("muted") {
      self.muted = muted;
    }
    if let Some(show_local) = flag("show_local") {
      self.show_local = show_local;
    }
    if let Some(opacity) = body.get("opacity").and_then(Value::as_f64) {
      self.opacity = opacity;
    }
    if let Some(page) = body.get("page").and_then(Value::as_str) {
      self.page = page.into();
    }
  }
}
