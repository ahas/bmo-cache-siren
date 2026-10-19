//! What the siren remembers of one session between its requests.

use std::collections::VecDeque;

use indexmap::IndexMap;

use crate::attribution::Flags;

/// How many message ids a session remembers: a streamed answer writes several lines with one id, and
/// one request is counted once.
pub const MSG_IDS_KEPT: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Session {
  /// Whether a request of this session has been seen, so the next one is not its start.
  pub seen: bool,
  /// Unix seconds of the previous request.
  pub last_ts: Option<f64>,
  pub last_model: Option<String>,
  pub last_ctx: u64,
  /// What the transcript said since the previous request.
  pub flags: Flags,
  /// The label of what prompted the next request.
  pub last_req: String,
  /// Tool-use id → tool name, to name the tool results that follow; oldest first, so the oldest are the
  /// ones let go.
  pub tool_names: IndexMap<String, String>,
  /// The newest message ids, at most `MSG_IDS_KEPT`.
  pub msg_ids: VecDeque<String>,
  pub agent_count: u64,
}

impl Session {
  /// Remember a message id; false when it was already remembered.
  pub fn remember(&mut self, msg_id: &str) -> bool {
    if self.msg_ids.iter().any(|seen| seen == msg_id) {
      return false;
    }
    if self.msg_ids.len() == MSG_IDS_KEPT {
      self.msg_ids.pop_front();
    }
    self.msg_ids.push_back(msg_id.to_string());
    true
  }
}
