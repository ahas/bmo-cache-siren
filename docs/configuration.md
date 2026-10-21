# Configuration

One JSON file, given with `--config` (default: `config.json` in the install root, which git ignores).
When the file is missing the daemon writes it with every default and runs on them. A file that cannot
be read, or gives a key a value of the wrong type, leaves the defaults in force and logs the failure.
Keys the file leaves out keep their defaults; keys it adds are carried but unused.

## Command line

`bmo-cache-siren serve` runs the daemon. Its flags:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--config <file>` | `config.json` in the install root | the config file below |
| `--data-dir <dir>` | `CACHE_SIREN_DATA_DIR`, else `~/.claude/cache-siren` | the data directory the siren writes (`docs/api.md`, Data directory) |
| `--token-file <file>` | `CACHE_SIREN_TOKEN_FILE`, else `token` in the data directory | the bearer token every API route takes; written with a new random token when missing |
| `--web-dir <dir>` | `web/` in the install root | the pages the daemon serves |
| `--quiet` | off | log lines go only to the log file, not to standard output |
| `--no-backfill` | off | skip the start-up load of recent requests; unread transcripts are taken from their end |

The install root is the nearest folder above the binary that holds `web/index.html`, so an installed
tree and a build inside the checkout both find their pages; the current directory where none does.

The other verbs and their flags are in `docs/api.md`, under the command.

## Keys

The file is read once, at start. `armed`, `ttl_s` and `red_new_tokens` can also change while running,
through `PATCH /settings` (`docs/api.md`), which applies them at once and merges them back into the
same file. Every other key changes only by editing the file and restarting.

`config.example.json` is a starting point for a host; it leaves out `projects_globs`,
`project_prefixes`, `siren_file` and `siren_player`, so those run on their defaults.

| Key | Default | Changes | Meaning |
| --- | --- | --- | --- |
| `port` | 9022 | start | loopback port the HTTP server binds |
| `poll_s` | 1.0 | start | seconds between transcript scans |
| `projects_globs` | `["~/.claude/projects"]` | start | transcript roots, as globs with `~` expanded; the default root is used when none matches |
| `project_prefixes` | `[]` | start | leading parts cut from an encoded project folder name to give the short project name; the first match wins |
| `recent_hours` | 6 | start | only transcripts modified within this many hours are tailed |
| `backfill_hours` | 3 | start | span of past requests loaded silently into the window at start (skipped with `--no-backfill`) |
| `yellow_new_tokens` | 20000 | start | new tokens that make a request yellow, and the line between warm and growth or cold |
| `red_new_tokens` | 100000 | start, and live as the settings' `threshold` | new tokens that make a cold or growth request red, which raises the siren |
| `burst_window_s` | 60 | start | the burst window in seconds |
| `burst_red_tokens` | 500000 | start | new tokens within the burst window that raise a burst siren |
| `siren_cooldown_s` | 20 | start | seconds a session's siren stays quiet after it was raised |
| `armed` | true | start, and live as the settings' `armed` | whether a raised siren sounds and speaks; quiet, it is still recorded |
| `ttl_s` | 3600 | start, and live as the settings' `ttlMs` | cache lifetime in seconds for a harness the table does not list; a gap must pass the lifetime for `ttl_expired` |
| `ttl_by_mode_model_s` | `claude-code`: `*` 3600; `codex`: `gpt-5.6` 1800; `openrouter`: empty | start | per harness name, the lifetime of the longest matching model prefix, else that harness's `*`; an empty table means no lifetime is judged for that harness |
| `prefix_reuse_min` | 0.5 | start | reading less than this share of the previous context means the prefix changed |
| `voice` | false | start | speak raised sirens |
| `voice_cmd` | `[]` | start | argument list run to speak, with `{text}` replaced by the alert text; nothing is spoken while empty |
| `sound` | true | start | play the siren sound |
| `siren_file` | `warning.mp3` in the data directory, as an absolute path | start | sound played when the file exists, with a leading `~` read as the home folder; else the generated `siren.wav` |
| `siren_player` | `[]` | start | argument list that plays the sound, with `{file}` replaced by the sound's path (appended where no argument holds it); empty chooses by host: `afplay` on macOS, elsewhere the first of `pw-play`, `paplay` and `aplay` on the `PATH` |
| `ring` | 600 | start | events held in the window and restored from the event log at start |
| `retention_days` | 7 | start | event-log lines older than this are pruned; 0 keeps everything |
| `prune_every_s` | 21600 | start | seconds between prunes, at least 600; the first runs 30 seconds after start |

## Deployment notes

- Transcripts are found only under `projects_globs`. A deployment whose harnesses write transcripts
  under their own Claude config directories, rather than under `~/.claude/projects`, lists those
  roots here; nothing else is scanned.
- Project names come out as the whole encoded folder name, less its leading dashes, unless
  `project_prefixes` names the leading part to cut, such as the encoded home directory.
- `aplay` plays only WAV files: a host whose one player is `aplay` gives `siren_file` a WAV, or leaves
  it missing so the generated `siren.wav` plays.
- The systemd unit passes `--data-dir` under the deployment's data root, so the config file and
  everything the daemon writes stand in that one folder, kept across releases; `~/.claude/cache-siren`
  is another installation's data directory and this unit writes nothing there.
