//! The `bmo-cache-siren` binary: `serve` runs the daemon (docs/configuration.md, Command line), `mcp` its
//! harness plugin's MCP server, `openapi` prints the contract, and every other verb asks the running daemon
//! through the client (docs/api.md, The command).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use cache_siren::cli;
use cache_siren::client::{DEFAULT_ADDRESS, SirenClient};
use cache_siren::clocks;
use cache_siren::paths::default_data_dir;
use cache_siren::siren::{Settings as SirenSettings, Siren, ensure_siren_wav};
use cache_siren::token::{read_or_make_token, read_token};
use cache_siren::wire::{EventQuery, SettingsPatch, StatsQuery};
use cache_siren::{App, Config, DataDir, Engine, Log, load_config, mcp, openapi, router};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(
  name = "bmo-cache-siren",
  version,
  about = "Live watch of prompt-cache hits and new tokens per request"
)]
struct Cli {
  #[command(flatten)]
  reach: Reach,
  /// What to do; `status` when none is given.
  #[command(subcommand)]
  command: Option<Verb>,
}

/// Where the service and its token are.
#[derive(Args)]
struct Reach {
  /// The service's base address.
  #[arg(long, global = true, env = "CACHE_SIREN_ADDRESS", default_value = DEFAULT_ADDRESS)]
  address: String,
  /// The service's bearer token itself.
  #[arg(long, global = true, env = "CACHE_SIREN_TOKEN", hide_env_values = true)]
  token: Option<String>,
  /// The file holding the bearer token [default: token in the data directory]
  #[arg(long, global = true, env = "CACHE_SIREN_TOKEN_FILE")]
  token_file: Option<PathBuf>,
  /// The data directory the daemon writes [default: ~/.claude/cache-siren]
  #[arg(long, global = true, env = "CACHE_SIREN_DATA_DIR")]
  data_dir: Option<PathBuf>,
}

impl Reach {
  fn data(&self) -> DataDir {
    DataDir::new(self.data_dir.clone().unwrap_or_else(default_data_dir))
  }

  fn token_file(&self) -> PathBuf {
    self.token_file.clone().unwrap_or_else(|| self.data().token())
  }

  /// The client's token: `--token`, else the token file's content.
  fn token(&self) -> Result<String, String> {
    if let Some(token) = &self.token {
      return Ok(token.clone());
    }
    let file = self.token_file();
    read_token(&file).map_err(|why| format!("the token file {} cannot be read: {why}", file.display()))
  }

  fn client(&self) -> Result<SirenClient, String> {
    Ok(SirenClient::new(&self.address, self.token()?))
  }
}

#[derive(Subcommand)]
enum Verb {
  /// Run the daemon: tail the transcripts, serve the pages and the API, sound the siren.
  Serve(Serve),
  /// The health, the settings and the sessions of the window, on one line.
  Status {
    #[arg(long)]
    json: bool,
  },
  /// The newest events.
  Tail {
    #[arg(default_value_t = 20)]
    events: u32,
    #[arg(long)]
    json: bool,
  },
  /// Cache hits and new tokens by session.
  Stats {
    /// Only events at or after this time, in epoch milliseconds.
    #[arg(long)]
    since: Option<i64>,
    /// Only this session.
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    json: bool,
  },
  /// The settings, or a change of the ones given.
  Settings {
    #[arg(long)]
    armed: Option<bool>,
    #[arg(long)]
    ttl_ms: Option<u64>,
    #[arg(long)]
    threshold: Option<u64>,
    #[arg(long)]
    json: bool,
  },
  /// Quiet the siren; sirens are still recorded.
  Mute {
    #[arg(long)]
    json: bool,
  },
  /// Arm the siren again.
  Unmute {
    #[arg(long)]
    json: bool,
  },
  /// Sound the siren and the voice once.
  Test {
    #[arg(long)]
    json: bool,
  },
  /// The live page's address with the token in its fragment.
  Page,
  /// Run the harness plugin's MCP server on standard input and output.
  Mcp,
  /// Print the OpenAPI document.
  Openapi,
  /// Restart the daemon under the host's service manager.
  Restart,
  /// The last lines of the daemon's log.
  Log {
    #[arg(default_value_t = 30)]
    lines: usize,
  },
}

#[derive(Args)]
struct Serve {
  /// The config file; created with the defaults when missing [default: config.json in the install root]
  #[arg(long)]
  config: Option<PathBuf>,
  /// The directory holding the pages [default: web/ in the install root]
  #[arg(long)]
  web_dir: Option<PathBuf>,
  /// Write log lines only to the log file, not to standard output
  #[arg(long)]
  quiet: bool,
  /// Skip the start-up load of recent requests; unread transcripts are taken from their end
  #[arg(long)]
  no_backfill: bool,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let reach = cli.reach;
  let verb = cli.command.unwrap_or(Verb::Status { json: false });
  let client = || reach.client();
  let settings_patch = |patch: SettingsPatch, json: bool| match client() {
    Ok(client) => {
      cli::print(client.change_settings(&patch), json, |settings| vec![cli::settings_line(settings)])
    }
    Err(why) => unreached(&why),
  };
  match verb {
    Verb::Serve(serve) => run(serve, &reach),
    Verb::Mcp => mcp_server(&reach),
    Verb::Openapi => {
      print!("{}", openapi::openapi_json());
      ExitCode::SUCCESS
    }
    Verb::Page => match reach.token() {
      Ok(token) => {
        println!("{}", cli::page_address(&reach.address, &token));
        ExitCode::SUCCESS
      }
      Err(why) => unreached(&why),
    },
    Verb::Restart => cli::restart(),
    Verb::Log { lines } => cli::log_tail(&reach.data().log(), lines),
    Verb::Settings {
      armed,
      ttl_ms,
      threshold,
      json,
    } if armed.is_some() || ttl_ms.is_some() || threshold.is_some() => settings_patch(
      SettingsPatch {
        armed,
        ttl_ms,
        threshold,
      },
      json,
    ),
    Verb::Mute { json } => settings_patch(
      SettingsPatch {
        armed: Some(false),
        ..SettingsPatch::default()
      },
      json,
    ),
    Verb::Unmute { json } => settings_patch(
      SettingsPatch {
        armed: Some(true),
        ..SettingsPatch::default()
      },
      json,
    ),
    verb => {
      let client = match client() {
        Ok(client) => client,
        Err(why) => return unreached(&why),
      };
      match verb {
        Verb::Status { json } => cli::status(&client, json),
        Verb::Tail { events, json } => cli::print(
          client.events(&EventQuery {
            limit: Some(events),
            ..EventQuery::default()
          }),
          json,
          cli::tail_lines,
        ),
        Verb::Stats {
          since,
          session,
          json,
        } => cli::print(
          client.stats(&StatsQuery {
            since,
            session_id: session,
          }),
          json,
          cli::stats_lines,
        ),
        Verb::Settings { json, .. } => {
          cli::print(client.settings(), json, |settings| vec![cli::settings_line(settings)])
        }
        Verb::Test { json } => cli::print(client.create_siren_test(), json, |test| {
          vec![if test.sounded { "sounded".into() } else { "raised, but quiet".into() }]
        }),
        _ => unreachable!("every other verb is answered above"),
      }
    }
  }
}

fn unreached(why: &str) -> ExitCode {
  eprintln!("bmo-cache-siren: {why}");
  ExitCode::FAILURE
}

fn mcp_server(reach: &Reach) -> ExitCode {
  let client = match reach.client() {
    Ok(client) => client,
    Err(why) => return unreached(&why),
  };
  let runtime = match tokio::runtime::Runtime::new() {
    Ok(runtime) => runtime,
    Err(why) => return unreached(&format!("no runtime: {why}")),
  };
  match runtime.block_on(mcp::serve(client)) {
    Ok(()) => ExitCode::SUCCESS,
    Err(why) => unreached(&format!("the MCP server stopped: {why}")),
  }
}

/// The install root: the nearest folder above the binary that holds the pages, so the same rule finds
/// an installed tree and a build in the checkout; the current directory where none does.
fn install_root() -> PathBuf {
  let exe = std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok());
  exe
    .as_deref()
    .and_then(Path::parent)
    .and_then(|dir| dir.ancestors().find(|dir| dir.join("web").join("index.html").is_file()))
    .map(Path::to_path_buf)
    .unwrap_or_else(|| PathBuf::from("."))
}

fn run(serve: Serve, reach: &Reach) -> ExitCode {
  let root = install_root();
  let data = reach.data();
  if let Err(why) = std::fs::create_dir_all(data.root()) {
    return unreached(&format!("the data directory {} cannot be made: {why}", data.root().display()));
  }
  let token_file = reach.token_file();
  let token = match read_or_make_token(&token_file) {
    Ok(token) => token,
    Err(why) => {
      return unreached(&format!("the token file {} cannot be read or made: {why}", token_file.display()));
    }
  };
  let log = Arc::new(Log::new(data.log(), serve.quiet));
  let config_path = serve.config.unwrap_or_else(|| root.join("config.json"));
  let config = load_config(&config_path, Config::defaults(data.root()), &log);
  log.info(
    "start",
    json!({"pid": std::process::id(), "port": config.port, "version": env!("CARGO_PKG_VERSION"),
           "cfg": {"yellow_new_tokens": config.yellow_new_tokens, "red_new_tokens": config.red_new_tokens,
                   "burst_red_tokens": config.burst_red_tokens, "poll_s": config.poll_s}}),
  );
  ensure_siren_wav(&data.siren_wav(), &log);

  let siren = Arc::new(Siren::on_host(SirenSettings::of(&config, &data), log.clone()));
  let port = config.port;
  let engine = Arc::new(Engine::new(config, data, log.clone(), siren));
  if serve.no_backfill {
    engine.skip_unread();
  } else {
    engine.backfill();
  }

  let web_dir = serve.web_dir.unwrap_or_else(|| root.join("web"));
  let app = Arc::new(App::new(engine.clone(), web_dir, Some(config_path), token));
  let stopping = app.clone();
  let runtime = tokio::runtime::Runtime::new().expect("a runtime");
  let served = runtime.block_on(async {
    clocks::start(engine.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    log.info("listening", json!({"addr": format!("127.0.0.1:{port}")}));
    // A stop ends the open streams, then waits for the answers under way to finish.
    let stop = async move {
      stopped().await;
      stopping.stop();
    };
    axum::serve(listener, router(app)).with_graceful_shutdown(stop).await
  });
  engine.save();
  match served {
    Ok(()) => {
      log.info("stop", json!({}));
      ExitCode::SUCCESS
    }
    Err(why) => {
      log.error("serve failed", json!({"err": why.to_string()}));
      ExitCode::FAILURE
    }
  }
}

/// Resolves on an interrupt or a termination request, so the offsets are kept before the process ends.
async fn stopped() {
  let interrupt = async {
    let _ = tokio::signal::ctrl_c().await;
  };
  #[cfg(unix)]
  let terminate = async {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
      Ok(mut signal) => {
        signal.recv().await;
      }
      Err(_) => std::future::pending::<()>().await,
    }
  };
  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();
  tokio::select! {
    () = interrupt => {}
    () = terminate => {}
  }
}
