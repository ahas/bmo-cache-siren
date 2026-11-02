//! The service's Rust client (docs/api.md, The client): one blocking call per route, in the wire types,
//! sending the token on every call. The command's verbs and the MCP server call the service through it.
//!
//! Its calls wait on their answers, so a caller on an async runtime makes them where waiting holds up no
//! other work.

use std::fmt;

use reqwest::Method;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::ACCEPT;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::wire::{
  Event, EventPage, EventQuery, Health, NewUsage, Settings, SettingsPatch, SirenTest, Stats, StatsQuery,
  WireError,
};

/// The address a client reaches where none is named.
pub const DEFAULT_ADDRESS: &str = "http://127.0.0.1:9022";

/// What a call came to short of the route's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientFailure {
  /// The service answered a failure; `answer` is its error body where it gave one.
  Answered {
    status: u16,
    answer: Option<WireError>,
  },
  /// No answer came, or it was not of the route's shape.
  Unreached(String),
}

impl fmt::Display for ClientFailure {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      ClientFailure::Answered {
        status,
        answer: Some(answer),
      } => write!(f, "{status} {}: {}", answer.code, answer.error),
      ClientFailure::Answered {
        status,
        answer: None,
      } => write!(f, "{status} with no error body"),
      ClientFailure::Unreached(why) => write!(f, "the service did not answer: {why}"),
    }
  }
}

impl std::error::Error for ClientFailure {}

/// The client of one service: its address and its bearer token.
#[derive(Clone, Debug)]
pub struct SirenClient {
  address: String,
  token: String,
  http: Client,
}

impl SirenClient {
  pub fn new(address: impl Into<String>, token: impl Into<String>) -> SirenClient {
    SirenClient {
      address: address.into().trim_end_matches('/').to_string(),
      token: token.into(),
      // A call waits for its answer however long the daemon takes, a siren test that plays and speaks
      // included: the blocking client's own default bound is taken off.
      http: Client::builder().timeout(None).build().expect("a plain HTTP client builds"),
    }
  }

  pub fn address(&self) -> &str {
    &self.address
  }

  fn request(&self, method: Method, path: &str) -> RequestBuilder {
    self
      .http
      .request(method, format!("{}{path}", self.address))
      .bearer_auth(&self.token)
      .header(ACCEPT, "application/json")
  }

  fn answer<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, ClientFailure> {
    let response = request.send().map_err(|why| ClientFailure::Unreached(why.to_string()))?;
    let status = response.status();
    let text = response.text().map_err(|why| ClientFailure::Unreached(why.to_string()))?;
    if !status.is_success() {
      return Err(ClientFailure::Answered {
        status: status.as_u16(),
        answer: serde_json::from_str(&text).ok(),
      });
    }
    serde_json::from_str(&text)
      .map_err(|why| ClientFailure::Unreached(format!("the answer did not read: {why}")))
  }

  fn get<T: DeserializeOwned>(&self, path: &str, query: &impl Serialize) -> Result<T, ClientFailure> {
    Self::answer(self.request(Method::GET, path).query(query))
  }

  /// `GET /health`.
  pub fn health(&self) -> Result<Health, ClientFailure> {
    self.get("/health", &())
  }

  /// `GET /events`, as a page.
  pub fn events(&self, query: &EventQuery) -> Result<EventPage, ClientFailure> {
    self.get("/events", query)
  }

  /// `POST /events`: the usage of one model request; the event it was recorded as.
  pub fn create_usage(&self, usage: &NewUsage) -> Result<Event, ClientFailure> {
    Self::answer(self.request(Method::POST, "/events").json(usage))
  }

  /// `GET /stats`.
  pub fn stats(&self, query: &StatsQuery) -> Result<Stats, ClientFailure> {
    self.get("/stats", query)
  }

  /// `GET /settings`.
  pub fn settings(&self) -> Result<Settings, ClientFailure> {
    self.get("/settings", &())
  }

  /// `PATCH /settings`.
  pub fn change_settings(&self, patch: &SettingsPatch) -> Result<Settings, ClientFailure> {
    Self::answer(self.request(Method::PATCH, "/settings").json(patch))
  }

  /// `POST /siren-tests`.
  pub fn create_siren_test(&self) -> Result<SirenTest, ClientFailure> {
    Self::answer(self.request(Method::POST, "/siren-tests"))
  }
}
