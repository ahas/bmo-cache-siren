# Porting

What runs anywhere, what is macOS only, what depends on outside services, and the least needed to
run on Linux. It is written so that a person or an agent setting the siren up on a new host can
follow it step by step.

In one line: the daemon and its browser page run on any OS with Python 3.11 or later; the overlay
app, the siren sound and autostart are per platform; the quota header and the heartbeat need
services of your own and are optional.

## The OS-independent core

- **The daemon** (`src/cache_siren.py`): standard library only. It tails transcripts under
  `projects_globs`, takes posted usage on `POST /events`, classifies each request and serves the
  page, the API and the event stream on `127.0.0.1:<port>`.
- **What it writes:** everything under `~/.claude/cache-siren/`: the log, the event log, the tail
  offsets, the overlay control file and the generated siren sound. `docs/api.md` lists each file.
- **The page** (`web/`): the live table, served by the daemon at `/`, with a frozen demo at `/demo`.
  Any browser shows it.
- **The table survives a restart:** at start the daemon restores the last `ring` rows from the event
  log, and the rolling five-hour window from the last five hours of it.
- **The CLI** (`bin/cache-siren`): bash, curl and the formatter in `src/cli_fmt.py`. Every command
  but `restart` works on any OS.

## macOS only, and the Linux replacement

| macOS | Linux replacement |
| --- | --- |
| The overlay app (`macos/`, Swift, an always-on-top panel showing the page) | an always-on-top browser window, such as `chromium --app=http://127.0.0.1:9022`, or a thin wrapper of your own |
| `afplay` for the siren | `paplay`, `aplay` or `ffplay -nodisp -autoexit`, which needs a change where the daemon plays the sound |
| launchd (`deploy/launchd/`) | systemd --user (`deploy/systemd/cache-siren.service`) |
| `bin/cache-siren restart` (launchctl) | `systemctl --user restart cache-siren` |
| `voice_cmd` | leave it empty or point it at your own text-to-speech command |

Only the macOS side has been verified by the original author. The Linux replacements are yours to
check; the core needs no change.

The launchd files carry two placeholders, replaced when they are installed:

- `@INSTALL_ROOT@`: the folder the repository is installed to, holding `src/` and `dist/`.
- `@HOME@`: the user's home folder, where the logs go.

Their labels are `local.cache-siren` (the daemon) and `local.cache-siren.app` (the overlay), and the
overlay's bundle identifier is `local.cache-siren`. launchctl addresses them in the user's GUI
domain, `gui/$(id -u)/<label>`, so no user id is written into any file.

The systemd unit reaches its install root through `%h`, the user's home, so it holds no user name
either.

## Outside services

### The quota source (`quota_url`)

The header's five-hour and seven-day percentages come from a JSON source you supply. Without one,
leave `quota_url` empty: the header shows `–` and the token watch and the sirens are unchanged. The
shape the daemon reads, and the order in which it chooses the slot to show, are in `docs/api.md`
under the quota source.

- A source that serves several accounts or providers marks the one supplying now; the daemon shows
  that slot.
- For a provider with no account login, the provider's own windows go in slot `"1"` with a null
  account. A provider with no windows shows `–`.
- The five-hour window is rolling: the percentage rises and falls without a reset. The daemon's own
  five-hour share is likewise computed over the last five hours.
- A trap when the source is fed by a status-line script: if every session writes a shared file every
  second, idle sessions overwrite fresh values with stale ones. Write only when that session's value
  has changed.

### The heartbeat (`heartbeat_url`)

Posted every 45 seconds to a service registry of your own. Leave it empty and nothing is sent; the
daemon still writes an alive line to its log every ten minutes, which a log-staleness watcher can use.

## The least needed on Linux

```
python3 src/cache_siren.py &
xdg-open http://127.0.0.1:9022/
bin/cache-siren tail 20
```

`config.json` is created with the defaults on first run. Set `projects_globs` if transcripts live
anywhere other than `~/.claude/projects`, empty `quota_url` and `heartbeat_url` if you have no such
services, and set `sound` to false until the siren player is replaced. `docs/configuration.md` lists
every key.

## Thresholds

The defaults (yellow at 20K new tokens, red at 100K, a burst at 500K within 60 seconds) come from
the original author's per-request distribution, where p95 sat near 12K and p99 near 170K.
Recalibrate them to your own: take the new tokens per request from your transcripts or from
`events.jsonl`, and set yellow a little above your p95 and red near your p99.
