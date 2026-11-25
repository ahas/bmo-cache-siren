//! What the service publishes beside its routes: the Rust client, the harness plugin's tools, exactly the
//! four the contract names, the OpenAPI document kept in docs/openapi.json, and the MCP server the binary
//! starts.

use crate::siren_server;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cache_siren::client::ClientFailure;
use cache_siren::mcp::{generator, tools};
use cache_siren::openapi::{openapi, openapi_json};
use cache_siren::wire::{EventQuery, NewUsage, SettingsPatch, StatsQuery};
use serde_json::{Value, json};
use siren_server::{SirenServer, TOKEN};

/// The tools the contract names, each with its route and its input's fields.
const TOOLS: [(&str, &str, &[&str]); 4] = [
  ("cache_stats", "GET /stats", &["since", "sessionId"]),
  ("cache_settings", "GET /settings", &[]),
  ("cache_settings_update", "PATCH /settings", &["armed", "ttlMs", "threshold"]),
  ("cache_events", "GET /events", &["after", "limit"]),
];

#[tokio::test(flavor = "multi_thread")]
async fn the_client_calls_every_route_and_reads_a_refusal() {
  let siren = SirenServer::start(None).await;
  let client = siren.client();
  tokio::task::spawn_blocking(move || {
    let client = client();
    assert_eq!(client.health().unwrap().version, cache_siren::VERSION);
    let posted = client
      .create_usage(&NewUsage {
        session_id: "thread-1".into(),
        model_name: "gpt-5.6-sol".into(),
        new_token_count: 10,
        at: 1,
        ..NewUsage::default()
      })
      .unwrap();
    assert_eq!(posted.id, 1);
    assert_eq!(client.events(&EventQuery::default()).unwrap().items, [posted]);
    assert_eq!(client.stats(&StatsQuery::default()).unwrap().sessions[0].session_id, "thread-1");
    let quiet = SettingsPatch {
      armed: Some(false),
      ..SettingsPatch::default()
    };
    assert!(!client.change_settings(&quiet).unwrap().armed);
    assert!(!client.settings().unwrap().armed);
    assert!(!client.create_siren_test().unwrap().sounded);
    let refused = client.events(&EventQuery {
      limit: Some(0),
      ..EventQuery::default()
    });
    assert!(
      matches!(&refused, Err(ClientFailure::Answered { status: 400, answer: Some(answer) }) if answer.code == "bad-request"),
      "{refused:?}"
    );
  })
  .await
  .unwrap();
}

#[test]
fn the_plugin_lists_exactly_the_contract_tools() {
  let listed = tools();
  let named: Vec<(&str, &str)> = listed.iter().map(|tool| (tool.name, tool.route)).collect();
  assert_eq!(named, TOOLS.map(|(name, route, _)| (name, route)));
  for (tool, (_, _, fields)) in listed.iter().zip(TOOLS) {
    let input = tool.input(&mut generator("/$defs")).to_value();
    let properties: Vec<&str> = input["properties"]
      .as_object()
      .map(|map| map.keys().map(String::as_str).collect())
      .unwrap_or_default();
    assert_eq!(properties, fields, "{}", tool.name);
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_is_one_call_through_the_client() {
  let siren = SirenServer::start(None).await;
  let client = siren.client();
  tokio::task::spawn_blocking(move || {
    let client = client();
    let listed = tools();
    let tool = |name: &str| listed.iter().find(|tool| tool.name == name).unwrap();
    let changed = tool("cache_settings_update").call(&client, json!({"threshold": 150_000})).unwrap();
    assert_eq!(changed["threshold"], json!(150_000));
    let events = tool("cache_events").call(&client, json!({"limit": 5})).unwrap();
    assert_eq!(events["items"][0]["kind"], json!("settings.changed"));
    let refused = tool("cache_settings_update").call(&client, json!({"threshold": 5})).unwrap_err();
    assert_eq!(refused.content()["code"], json!("conflict"));
    let wrong = tool("cache_stats").call(&client, json!({"since": "yesterday"})).unwrap_err();
    assert_eq!(wrong.content()["code"], json!("bad-request"));
  })
  .await
  .unwrap();
}

#[test]
fn the_kept_openapi_document_is_the_one_the_code_writes() {
  let kept = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/openapi.json")).unwrap();
  assert!(
    kept == openapi_json(),
    "docs/openapi.json differs from what the code writes; write it again with \
     cargo run -- openapi > docs/openapi.json"
  );
  let document = openapi();
  assert_eq!(document["info"]["version"], json!(env!("CARGO_PKG_VERSION")));
  assert_eq!(document["x-calls"], json!([]));
  assert_eq!(document["x-harness"]["mcp"], json!("bmo-cache-siren mcp"));
  let paths: Vec<&String> = document["paths"].as_object().unwrap().keys().collect();
  assert_eq!(paths, ["/health", "/events", "/stats", "/settings", "/siren-tests"]);
}

/// A child process taken down, with what it started, when the test lets go of it.
struct Running(Child);

impl Drop for Running {
  fn drop(&mut self) {
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_the_tools_over_mcp() {
  let siren = SirenServer::start(None).await;
  let child = Command::new(env!("CARGO_BIN_EXE_bmo-cache-siren"))
    .args(["mcp", "--address", &siren.url(""), "--token", TOKEN])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
  let mut running = Running(child);
  let mut input = running.0.stdin.take().unwrap();
  let output = running.0.stdout.take().unwrap();
  let talk = tokio::task::spawn_blocking(move || {
    let asks = [
      json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "tester", "version": "1"}}}),
      json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
      json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
      json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "cache_settings", "arguments": {}}}),
    ];
    for ask in asks {
      writeln!(input, "{ask}").unwrap();
    }
    input.flush().unwrap();
    let mut answers: Vec<Value> = Vec::new();
    for line in BufReader::new(output).lines() {
      let answer: Value = serde_json::from_str(&line.unwrap()).unwrap();
      answers.push(answer);
      // Requests are answered as each finishes, so the three answers may come in any order.
      if answers.iter().filter(|answer| answer["id"].is_i64()).count() == 3 {
        break;
      }
    }
    drop(input);
    answers
  });
  let answers = tokio::time::timeout(Duration::from_secs(20), talk)
    .await
    .expect("the server answers")
    .unwrap();
  let by_id = |id: i64| {
    let found = answers.iter().find(|answer| answer["id"] == json!(id));
    found.unwrap_or_else(|| panic!("no answer {id} among {answers:?}"))
  };

  assert_eq!(by_id(1)["result"]["serverInfo"]["version"], json!(cache_siren::VERSION));
  let listed: Vec<&Value> = by_id(2)["result"]["tools"]
    .as_array()
    .unwrap()
    .iter()
    .map(|tool| &tool["name"])
    .collect();
  assert_eq!(listed, TOOLS.map(|(name, _, _)| json!(name)).iter().collect::<Vec<_>>());
  assert_eq!(
    by_id(3)["result"]["structuredContent"],
    json!({"armed": true, "ttlMs": 3_600_000, "threshold": 100_000})
  );
  let ended = running.0.wait().unwrap();
  assert!(ended.success(), "closing its input ends the server: {ended}");
}
