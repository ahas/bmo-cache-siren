//! The `cache-siren` binary: `serve` runs the daemon (docs/configuration.md, Command line); every other
//! verb asks the running daemon, on the port `CACHE_SIREN_PORT` names or 9022.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use cache_siren::cli::{self, Cli as Daemon};
use cache_siren::clocks;
use cache_siren::paths::default_data_dir;
use cache_siren::siren::{Settings, Siren, ensure_siren_wav};
use cache_siren::{App, Config, DataDir, Engine, Log, load_config, router};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(
  name = "cache-siren",
  about = "Live watch of prompt-cache hits and new tokens per request"
)]
struct Cli {
  /// What to do; `status` when none is given.
  #[command(subcommand)]
  command: Option<Verb>,
}

#[derive(Subcommand)]
enum Verb {
  /// Run the daemon: tail the transcripts, serve the page and the API, sound the siren.
  Serve(Serve),
  /// Whether the daemon is up, with its files, events, newest row and 5-hour quota.
  Status,
  /// Show the overlay.
  Show,
  /// Hide the overlay.
  Hide,
  /// Show the overlay if hidden, else hide it.
  Toggle,
  /// Silence the siren and the voice; alerts are still logged.
  Mute,
  /// Let the siren and the voice sound again.
  Unmute,
  /// Show local-backend rows (`on`) or leave them out (anything else).
  Local { state: Option<String> },
  /// Switch the overlay to the frozen demo page.
  Demo,
  /// Switch the overlay back to the live page.
  Live,
  /// Sound the siren and the voice once.
  Test,
  /// The newest rows, local ones included.
  Tail {
    #[arg(default_value = "20")]
    rows: String,
  },
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
  /// The data directory the siren writes [default: ~/.claude/cache-siren]
  #[arg(long)]
  data_dir: Option<PathBuf>,
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
  let verb = Cli::parse().command.unwrap_or(Verb::Status);
  let port = std::env::var("CACHE_SIREN_PORT")
    .ok()
    .and_then(|port| port.parse().ok())
    .unwrap_or(9022);
  let daemon = Daemon::new(port);
  match verb {
    Verb::Serve(serve) => {
      run(serve);
      ExitCode::SUCCESS
    }
    Verb::Status => daemon.status(),
    Verb::Show => daemon.show(true),
    Verb::Hide => daemon.show(false),
    Verb::Toggle => daemon.toggle(),
    Verb::Mute => daemon.mute(true),
    Verb::Unmute => daemon.mute(false),
    Verb::Local { state } => daemon.local(state.as_deref() == Some("on")),
    Verb::Demo => daemon.demo(),
    Verb::Live => daemon.live(),
    Verb::Test => daemon.test(),
    Verb::Tail { rows } => daemon.tail(&rows),
    Verb::Restart => cli::restart(),
    Verb::Log { lines } => cli::log_tail(&DataDir::new(default_data_dir()).log(), lines),
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

fn run(serve: Serve) {
  let root = install_root();
  let data = DataDir::new(serve.data_dir.unwrap_or_else(default_data_dir));
  if let Err(why) = std::fs::create_dir_all(data.root()) {
    eprintln!("cache-siren: the data directory {} cannot be made: {why}", data.root().display());
    std::process::exit(1);
  }
  let log = Arc::new(Log::new(data.log(), serve.quiet));
  let config_path = serve.config.unwrap_or_else(|| root.join("config.json"));
  let config = load_config(&config_path, Config::defaults(data.root()), &log);
  log.info(
    "start",
    json!({"pid": std::process::id(), "port": config.port,
           "cfg": {"yellow_new_tokens": config.yellow_new_tokens, "red_new_tokens": config.red_new_tokens,
                   "burst_red_tokens": config.burst_red_tokens, "poll_s": config.poll_s}}),
  );
  ensure_siren_wav(&data.siren_wav(), &log);

  let siren = Arc::new(Siren::on_host(Settings::of(&config, &data), log.clone()));
  let port = config.port;
  let engine = Arc::new(Engine::new(config, data, log.clone(), siren));
  if serve.no_backfill {
    engine.skip_unread();
  } else {
    engine.backfill();
  }
  engine.seed_buckets();

  let web_dir = serve.web_dir.unwrap_or_else(|| root.join("web"));
  let app = Arc::new(App::new(engine.clone(), web_dir, Some(config_path)));
  let runtime = tokio::runtime::Runtime::new().expect("a runtime");
  let served = runtime.block_on(async {
    clocks::start(engine.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    log.info("listening", json!({"addr": format!("127.0.0.1:{port}")}));
    axum::serve(listener, router(app)).with_graceful_shutdown(stopped()).await
  });
  engine.save();
  match served {
    Ok(()) => log.info("stop", json!({})),
    Err(why) => {
      log.error("serve failed", json!({"err": why.to_string()}));
      std::process::exit(1);
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
