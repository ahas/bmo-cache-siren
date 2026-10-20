# Porting

What runs anywhere, what is macOS only, and the least needed to run on Linux. It is written so that a
person or an agent setting the siren up on a new host can follow it step by step.

In one line: the daemon, its client, its MCP server and its browser page run on any OS a Rust toolchain
builds for; the overlay app, the siren player and autostart are per platform. The siren calls no other
service, so nothing else has to be running.

## The OS-independent core

- **The daemon** (`bmo-cache-siren serve`, built from `src/`): one binary of registry crates. It tails
  transcripts under `projects_globs`, takes posted usage on `POST /events`, classifies each request
  and serves the pages, the API and the event stream on `127.0.0.1:<port>`.
- **What it writes:** everything under `~/.claude/cache-siren/`, or the directory `--data-dir` names:
  the log, the event log, the tail offsets, the API token and the generated siren sound. `docs/api.md`
  lists each file.
- **The page** (`web/`): the live table, served by the daemon at `/`, with a frozen demo at `/demo`.
  Any browser shows it, opened at the address `bmo-cache-siren page` prints.
- **The table survives a restart:** at start the daemon restores the newest `ring` events from the
  event log, and their numbering carries on.
- **The command** (the same binary's other verbs) and **the MCP server** (`bmo-cache-siren mcp`): every
  verb works on any OS; `restart` asks launchd on macOS and systemd --user elsewhere.

## macOS only, and the Linux replacement

| macOS | Linux replacement |
| --- | --- |
| The overlay app (`macos/`, Swift, an always-on-top panel showing the page) | an always-on-top browser window, such as `chromium --app="$(bmo-cache-siren page)"`, or a thin wrapper of your own |
| `afplay` for the siren | the first of `pw-play`, `paplay` and `aplay` on the `PATH`, chosen at start; `siren_player` names another |
| launchd (`deploy/launchd/`) | systemd --user (`deploy/systemd/cache-siren.service`) |
| `bmo-cache-siren restart` runs `launchctl kickstart` | `bmo-cache-siren restart` runs `systemctl --user restart cache-siren` |
| `voice_cmd` | leave it empty or point it at your own text-to-speech command |

The overlay app reads the API token from `CACHE_SIREN_TOKEN`, else from the token file in the data
directory (`CACHE_SIREN_DATA_DIR`, `CACHE_SIREN_TOKEN_FILE`), and loads the page at
`CACHE_SIREN_ADDRESS` with the token in its fragment.

The launchd files carry two placeholders, replaced when they are installed:

- `@INSTALL_ROOT@`: the folder the siren is installed to, holding `bin/bmo-cache-siren`, `web/` and
  `dist/`.
- `@HOME@`: the user's home folder, where the logs go.

Their labels are `local.cache-siren` (the daemon) and `local.cache-siren.app` (the overlay), and the
overlay's bundle identifier is `local.cache-siren`. launchctl addresses them in the user's GUI
domain, `gui/$(id -u)/<label>`, so no user id is written into any file.

The systemd unit reaches its deploy tree through `%h`, the user's home, so it holds no user name
either. It runs the deployed release's `bin/bmo-cache-siren serve` with the config file kept in the
deployment's data root (`testbed.toml`).

## The least needed on Linux

```
cargo build --release
target/release/bmo-cache-siren serve &
xdg-open "$(target/release/bmo-cache-siren page)"
target/release/bmo-cache-siren tail 20
```

`config.json` is created with the defaults on first run. Set `projects_globs` if transcripts live
anywhere other than `~/.claude/projects`. `docs/configuration.md` lists every key. The daemon writes
an alive line to its log every ten minutes, which a log-staleness watcher can use.

## Thresholds

The defaults (yellow at 20K new tokens, red at 100K, a burst at 500K within 60 seconds) come from
the original author's per-request distribution, where p95 sat near 12K and p99 near 170K.
Recalibrate them to your own: take the new tokens per request from your transcripts, from
`events.jsonl` or from `bmo-cache-siren stats`, and set yellow a little above your p95 and red near your
p99; the red line also changes live through `bmo-cache-siren settings --threshold`.
