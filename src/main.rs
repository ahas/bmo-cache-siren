//! The `cache-siren` binary: `serve` runs the daemon (docs/configuration.md, Command line).

use std::path::{Path, PathBuf};
use std::sync::Arc;

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
  #[command(subcommand)]
  command: Verb,
}

#[derive(Subcommand)]
enum Verb {
  /// Run the daemon: tail the transcripts, serve the page and the API, sound the siren.
  Serve(Serve),
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

fn main() {
  let cli = Cli::parse();
  match cli.command {
    Verb::Serve(serve) => run(serve),
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
