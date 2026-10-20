//! The service's OpenAPI 3.1 document (docs/api.md): every route with its parameters, its body, its answers
//! and the codes it can answer, the bodies' schemas written from the wire types, the events, the pages, and
//! the harness plugin's tools from the table the MCP server lists. The repository keeps it as
//! `docs/openapi.json`, and a test holds the kept document to this one.

use schemars::JsonSchema;
use schemars::generate::SchemaGenerator;
use serde_json::{Map, Value, json};

use crate::mcp::{ENTRY, MCP, PLUGIN, generator, tools};
use crate::wire::{
  EVENT_KINDS, Event, EventPage, EventQuery, Health, NewUsage, Settings, SettingsPatch, SirenTest, Stats,
  StatsQuery, WireError,
};

/// Where the schemas the routes refer to stand in the document.
const SCHEMAS: &str = "/components/schemas";

const DESCRIPTION: &str = "A live watch of prompt-cache hits and new tokens per model request, sounding a siren \
  when new tokens jump. It tails harness transcripts and takes the usage a runner posts; a watched session is a \
  harness session named by sessionId with its modelName. It calls no other service. Bodies are JSON with \
  camelCase keys; times are integers of epoch milliseconds. A page is items and more. Every failure is answered \
  in the error shape with its status. The pages and their assets are its own screen (x-pages), outside the API. \
  The harness plugin's tools, each a thin call of one route, are stated under x-tools.";

/// Each code a route can answer, its status and what it means.
const CODES: &[(&str, u16, &str)] = &[
  ("bad-request", 400, "the body or the query is not the shape the route takes"),
  ("unauthorized", 401, "the bearer token is missing or wrong"),
  ("no-route", 404, "no route answers this method and path"),
  ("conflict", 409, "a settings change the settings as they stand do not allow"),
  ("gone", 410, "a resume from before the event window"),
  ("too-large", 413, "the body is over 65536 bytes"),
  ("failed", 500, "the service failed"),
];

/// One route as the document states it.
struct Route {
  method: &'static str,
  path: &'static str,
  id: &'static str,
  tag: &'static str,
  summary: &'static str,
  parameters: Vec<Value>,
  body: Option<Value>,
  /// The success's status and content.
  answer: (u16, Value),
  /// The codes of its own, beside the token's.
  codes: Vec<&'static str>,
}

impl Route {
  fn operation(self) -> Value {
    let mut codes = vec!["unauthorized"];
    codes.extend(self.codes);
    let mut responses = Map::new();
    let (status, content) = self.answer;
    responses.insert(status.to_string(), json!({ "description": self.summary, "content": content }));
    responses.insert("default".into(), json!({ "$ref": "#/components/responses/Error" }));
    let mut operation = json!({
      "operationId": self.id,
      "tags": [self.tag],
      "summary": self.summary,
      "responses": responses,
      "x-codes": codes,
    });
    if !self.parameters.is_empty() {
      operation["parameters"] = self.parameters.into();
    }
    if let Some(body) = self.body {
      operation["requestBody"] =
        json!({ "required": true, "content": { "application/json": { "schema": body } } });
    }
    operation
  }
}

fn json_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Value {
  json!({ "application/json": { "schema": generator.subschema_for::<T>() } })
}

/// The query parameters of a query type, each property one parameter.
fn query<Query: JsonSchema>(generator: &mut SchemaGenerator) -> Vec<Value> {
  let schema = Query::json_schema(generator).to_value();
  let Some(properties) = schema["properties"].as_object() else {
    return Vec::new();
  };
  properties
    .iter()
    .map(|(name, property)| {
      let mut parameter = json!({ "name": name, "in": "query", "required": false, "schema": property });
      if let Some(description) = property.get("description") {
        parameter["description"] = description.clone();
      }
      parameter
    })
    .collect()
}

fn routes(generator: &mut SchemaGenerator) -> Vec<Route> {
  let mut event_parameters = query::<EventQuery>(generator);
  event_parameters.push(json!({
    "name": "access_token", "in": "query", "required": false, "schema": { "type": "string" },
    "description": "The token, for a reader that cannot set a header (RFC 6750, section 2.3).",
  }));
  let stream = json!({
    "text/event-stream": {
      "schema": { "type": "string" },
      "x-event": generator.subschema_for::<Event>(),
    },
  });
  let mut events_answer = json_of::<EventPage>(generator);
  if let (Value::Object(answer), Value::Object(stream)) = (&mut events_answer, stream) {
    answer.extend(stream);
  }
  vec![
    Route {
      method: "get",
      path: "/health",
      id: "listHealth",
      tag: "health",
      summary: "Readiness and version.",
      parameters: Vec::new(),
      body: None,
      answer: (200, json_of::<Health>(generator)),
      codes: Vec::new(),
    },
    Route {
      method: "get",
      path: "/events",
      id: "listEvents",
      tag: "events",
      summary: "The one stream: each request's usage, sirens, settings changes (SSE), or a page of it to a \
        reader asking Accept: application/json.",
      parameters: event_parameters,
      body: None,
      answer: (200, events_answer),
      codes: vec!["bad-request", "gone"],
    },
    Route {
      method: "post",
      path: "/events",
      id: "createEvents",
      tag: "events",
      summary: "A runner posts the usage of one model request.",
      parameters: Vec::new(),
      body: Some(generator.subschema_for::<NewUsage>().to_value()),
      answer: (201, json_of::<Event>(generator)),
      codes: vec!["bad-request"],
    },
    Route {
      method: "get",
      path: "/stats",
      id: "listStats",
      tag: "stats",
      summary: "Cache hits and new tokens by session since a time.",
      parameters: query::<StatsQuery>(generator),
      body: None,
      answer: (200, json_of::<Stats>(generator)),
      codes: vec!["bad-request"],
    },
    Route {
      method: "get",
      path: "/settings",
      id: "listSettings",
      tag: "settings",
      summary: "The siren's switch, the cache lifetime it assumes and its threshold.",
      parameters: Vec::new(),
      body: None,
      answer: (200, json_of::<Settings>(generator)),
      codes: Vec::new(),
    },
    Route {
      method: "patch",
      path: "/settings",
      id: "changeSettings",
      tag: "settings",
      summary: "Arm or quiet the siren, or change its lifetime or threshold.",
      parameters: Vec::new(),
      body: Some(generator.subschema_for::<SettingsPatch>().to_value()),
      answer: (200, json_of::<Settings>(generator)),
      codes: vec!["bad-request", "conflict"],
    },
    Route {
      method: "post",
      path: "/siren-tests",
      id: "createSirenTests",
      tag: "siren-tests",
      summary: "Sound the siren once to test it.",
      parameters: Vec::new(),
      body: None,
      answer: (200, json_of::<SirenTest>(generator)),
      codes: Vec::new(),
    },
  ]
}

/// The OpenAPI document.
pub fn openapi() -> Value {
  let mut generator = generator(SCHEMAS);
  let mut paths = Map::new();
  for route in routes(&mut generator) {
    let (path, method) = (route.path, route.method);
    paths.entry(path).or_insert_with(|| json!({}))[method] = route.operation();
  }
  let x_tools: Vec<Value> = tools()
    .iter()
    .map(|tool| {
      json!({
        "name": tool.name,
        "description": tool.description,
        "input": tool.input(&mut generator),
        "output": tool.output(&mut generator),
        "route": tool.route,
      })
    })
    .collect();
  let error = generator.subschema_for::<WireError>();
  let codes: Vec<String> = CODES
    .iter()
    .map(|(code, status, meaning)| format!("- `{code}` ({status}): {meaning}"))
    .collect();
  json!({
    "openapi": "3.1.0",
    "info": { "title": "bmo-cache-siren", "version": env!("CARGO_PKG_VERSION"), "description": DESCRIPTION },
    "servers": [{ "url": "/", "description": "the cache-siren service" }],
    "security": [{ "bearer": [] }],
    "x-layer": "upstream",
    "x-stage": 2,
    "x-calls": [],
    "x-harness": { "plugin": PLUGIN, "mcp": MCP, "entry": ENTRY },
    "tags": [
      { "name": "health" }, { "name": "events" }, { "name": "stats" }, { "name": "settings" },
      { "name": "siren-tests" },
    ],
    "paths": paths,
    "components": {
      "schemas": generator.take_definitions(true),
      "responses": {
        "Error": {
          "description": format!("A failure, in the common error shape. The codes:\n{}", codes.join("\n")),
          "content": { "application/json": { "schema": error } },
        },
      },
      "securitySchemes": {
        "bearer": {
          "type": "http",
          "scheme": "bearer",
          "description": "The content of the service's token file.",
        },
      },
    },
    "x-events": EVENT_KINDS,
    "x-tools": x_tools,
    "x-pages": ["cache-siren GET /", "cache-siren GET /demo", "cache-siren GET /asset/{name}"],
  })
}

/// The document as the repository keeps it: two-space indented JSON with a closing newline.
pub fn openapi_json() -> String {
  let mut text = serde_json::to_string_pretty(&openapi()).unwrap_or_default();
  text.push('\n');
  text
}
