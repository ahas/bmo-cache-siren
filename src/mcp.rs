//! `bmo-cache-siren mcp`: the harness plugin's MCP server on standard input and output (docs/api.md, The
//! harness plugin). Each tool is a thin call of one route through the service's client; the server lists
//! them and the OpenAPI document states them under `x-tools`, both from the one table here.

use std::sync::Arc;

use rmcp::model::{
  CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, InitializeResult,
  JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::{JsonSchema, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::{ClientFailure, SirenClient};
use crate::wire::{EventPage, EventQuery, Settings, SettingsPatch, Stats, StatsQuery};

/// The plugin a harness takes the siren in, and the command that starts its MCP server.
pub const PLUGIN: &str = "bmo-cache-siren";
pub const MCP: &str = "bmo-cache-siren mcp";

/// What a harness does first with it, which the server tells a harness when it starts.
pub const ENTRY: &str = "A harness reads how well its own prompt cache is holding with cache_stats and widens \
  or narrows the siren with cache_settings_update.";

/// What a tool's call came to short of the route's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolFailure {
  /// The arguments are not the shape the tool takes.
  Arguments(String),
  /// The call reached no answer of the route's shape.
  Client(ClientFailure),
}

impl ToolFailure {
  /// The failure as the tool error's content: the service's status, code and error, or the sentence
  /// saying why nothing answered.
  pub fn content(&self) -> Value {
    match self {
      ToolFailure::Arguments(why) => json!({ "code": "bad-request", "error": why }),
      ToolFailure::Client(ClientFailure::Answered {
        status,
        answer: Some(answer),
      }) => json!({ "status": status, "code": answer.code, "error": answer.error }),
      ToolFailure::Client(failure @ ClientFailure::Answered { status, .. }) => {
        json!({ "status": status, "error": failure.to_string() })
      }
      ToolFailure::Client(failure) => json!({ "error": failure.to_string() }),
    }
  }
}

type Call = Box<dyn Fn(&SirenClient, Value) -> Result<Value, ToolFailure> + Send + Sync>;

/// One tool: its name, what it does, the route it calls, its input and output, and the call itself.
pub struct SirenTool {
  pub name: &'static str,
  pub description: &'static str,
  /// The route the tool calls, as its method and path.
  pub route: &'static str,
  input: fn(&mut SchemaGenerator) -> Schema,
  output: fn(&mut SchemaGenerator) -> Schema,
  call: Call,
}

impl SirenTool {
  /// The tool's input, its nested types referred to where `generator` keeps its definitions.
  pub fn input(&self, generator: &mut SchemaGenerator) -> Schema {
    (self.input)(generator)
  }

  /// The tool's output, referred to where `generator` keeps its definitions.
  pub fn output(&self, generator: &mut SchemaGenerator) -> Schema {
    (self.output)(generator)
  }

  /// Calls the tool's route through `client` with the tool's arguments; it waits on the answer.
  pub fn call(&self, client: &SirenClient, arguments: Value) -> Result<Value, ToolFailure> {
    (self.call)(client, arguments)
  }
}

/// A tool whose input is `I` and whose answer is `O`, calling `call` with the arguments read as `I`.
fn tool<I, O, F>(name: &'static str, description: &'static str, route: &'static str, call: F) -> SirenTool
where
  I: DeserializeOwned + JsonSchema,
  O: Serialize + JsonSchema,
  F: Fn(&SirenClient, I) -> Result<O, ClientFailure> + Send + Sync + 'static,
{
  SirenTool {
    name,
    description,
    route,
    input: I::json_schema,
    output: SchemaGenerator::subschema_for::<O>,
    call: Box::new(move |client, arguments| {
      let input = serde_json::from_value::<I>(arguments).map_err(|error| {
        ToolFailure::Arguments(format!("The arguments are not the shape this tool takes: {error}."))
      })?;
      let answer = call(client, input).map_err(ToolFailure::Client)?;
      serde_json::to_value(answer).map_err(|why| ToolFailure::Arguments(why.to_string()))
    }),
  }
}

/// The sums asked of `cache_stats`.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatsInput {
  /// Only events at or after this time, in epoch milliseconds; the oldest event held where absent.
  #[serde(default)]
  since: Option<i64>,
  /// Only this session.
  #[serde(default)]
  session_id: Option<String>,
}

/// Nothing: `cache_settings` takes no input.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoInput {}

/// The settings to change; a field left out or null stays.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsInput {
  /// Arm the siren (true) or quiet it (false).
  #[serde(default)]
  armed: Option<bool>,
  /// The cache lifetime in milliseconds for a harness the per-harness table does not list.
  #[serde(default)]
  ttl_ms: Option<u64>,
  /// The new tokens that make a cold or growth request red.
  #[serde(default)]
  threshold: Option<u64>,
}

/// The page asked of `cache_events`.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventsInput {
  /// An event number: only events after it. The newest events where absent.
  #[serde(default)]
  after: Option<i64>,
  /// 1 to 1000; 100 where absent.
  #[serde(default)]
  limit: Option<u32>,
}

/// Every tool the server lists, in the order the OpenAPI document states them.
pub fn tools() -> Vec<SirenTool> {
  vec![
    tool::<StatsInput, Stats, _>(
      "cache_stats",
      "Cache hits and new tokens by session since a time.",
      "GET /stats",
      |client, asked| {
        client.stats(&StatsQuery {
          since: asked.since,
          session_id: asked.session_id,
        })
      },
    ),
    tool::<NoInput, Settings, _>(
      "cache_settings",
      "The siren's switch, the cache lifetime it assumes and its threshold.",
      "GET /settings",
      |client, _| client.settings(),
    ),
    tool::<SettingsInput, Settings, _>(
      "cache_settings_update",
      "Arm or quiet the siren, or change its lifetime or threshold.",
      "PATCH /settings",
      |client, asked| {
        client.change_settings(&SettingsPatch {
          armed: asked.armed,
          ttl_ms: asked.ttl_ms,
          threshold: asked.threshold,
        })
      },
    ),
    tool::<EventsInput, EventPage, _>(
      "cache_events",
      "The latest usage and siren events, a page after a cursor.",
      "GET /events",
      |client, asked| {
        client.events(&EventQuery {
          after: asked.after,
          limit: asked.limit,
          ..EventQuery::default()
        })
      },
    ),
  ]
}

/// The generator every tool schema and the OpenAPI document are written with: JSON Schema 2020-12, read as
/// a caller sends it, definitions kept at `definitions_path`.
pub fn generator(definitions_path: &'static str) -> SchemaGenerator {
  SchemaSettings::draft2020_12()
    .for_deserialize()
    .with(|settings| {
      settings.definitions_path = definitions_path.into();
      settings.meta_schema = None;
    })
    .into_generator()
}

/// The server over the service the client reaches.
pub struct SirenServer {
  client: SirenClient,
  tools: Arc<Vec<SirenTool>>,
}

impl SirenServer {
  pub fn new(client: SirenClient) -> SirenServer {
    SirenServer {
      client,
      tools: Arc::new(tools()),
    }
  }

  /// The tools as MCP lists them, each input and output a schema of its own with its definitions inside.
  pub fn listed(&self) -> Vec<Tool> {
    self.tools.iter().map(listed).collect()
  }
}

fn listed(tool: &SirenTool) -> Tool {
  let mut generator = generator("/$defs");
  let input = tool.input(&mut generator);
  let output = tool.output(&mut generator);
  let definitions = generator.take_definitions(true);
  let mut listed = Tool::new(tool.name, tool.description, standing_alone(input, &definitions));
  listed.output_schema = Some(Arc::new(standing_alone(output, &definitions)));
  listed
}

/// A schema made to stand on its own: an object at its root, with every definition it refers to beside it.
fn standing_alone(schema: Schema, definitions: &Map<String, Value>) -> JsonObject {
  let mut object = match schema.to_value() {
    Value::Object(object) if object.contains_key("$ref") => {
      let mut wrapped = Map::new();
      wrapped.insert("allOf".into(), Value::Array(vec![Value::Object(object)]));
      wrapped.insert("type".into(), "object".into());
      wrapped
    }
    Value::Object(object) => object,
    _ => Map::from_iter([("type".to_owned(), Value::from("object"))]),
  };
  if !definitions.is_empty() {
    object.insert("$defs".into(), Value::Object(definitions.clone()));
  }
  object
}

impl ServerHandler for SirenServer {
  fn get_info(&self) -> InitializeResult {
    let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
    let mut server = Implementation::default();
    server.name = "bmo-cache-siren".to_owned();
    server.version = crate::VERSION.to_owned();
    info.server_info = server;
    info.instructions = Some(ENTRY.to_owned());
    info
  }

  async fn list_tools(
    &self,
    _: Option<PaginatedRequestParams>,
    _: RequestContext<RoleServer>,
  ) -> Result<ListToolsResult, ErrorData> {
    Ok(ListToolsResult::with_all_items(self.listed()))
  }

  async fn call_tool(
    &self,
    request: CallToolRequestParams,
    _: RequestContext<RoleServer>,
  ) -> Result<CallToolResponse, ErrorData> {
    if !self.tools.iter().any(|tool| tool.name == request.name) {
      return Err(ErrorData::invalid_params(format!("There is no tool {}.", request.name), None));
    }
    let arguments = Value::Object(request.arguments.unwrap_or_default());
    let (client, tools, name) = (self.client.clone(), Arc::clone(&self.tools), request.name.to_string());
    // The client waits on its answer, so the call runs where waiting holds up no other session's work.
    let answered = tokio::task::spawn_blocking(move || {
      let tool = tools.iter().find(|tool| tool.name == name).expect("the tool was found above");
      tool.call(&client, arguments)
    })
    .await
    .map_err(|why| ErrorData::internal_error(why.to_string(), None))?;
    let result = match answered {
      Ok(answer) => CallToolResult::structured(answer),
      Err(failure) => CallToolResult::error(vec![ContentBlock::text(failure.content().to_string())]),
    };
    Ok(CallToolResponse::Complete(result))
  }
}

/// Serve the tools on standard input and output until the harness closes them.
pub async fn serve(client: SirenClient) -> Result<(), String> {
  use rmcp::ServiceExt;
  let running = SirenServer::new(client)
    .serve(rmcp::transport::stdio())
    .await
    .map_err(|why| why.to_string())?;
  running.waiting().await.map(|_| ()).map_err(|why| why.to_string())
}
