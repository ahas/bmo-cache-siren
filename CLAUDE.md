# Working in bmo-cache-siren

A daemon that watches prompt-cache hits and new tokens per model request and sounds a siren when new
tokens jump. `README.md` is the overview; `docs/` is canonical for the API, the configuration and
porting. Read the document for an area before changing it.

## Layout

- `src/cache_siren.py`: the daemon (tailing, classification, cause attribution, HTTP and the
  stream); `src/cli_fmt.py`: the CLI's output formatter.
- `bin/cache-siren`: the CLI, bash and curl against the running daemon.
- `web/`: the live page and the demo page the daemon serves.
- `macos/`: the Swift overlay app and its build script.
- `deploy/`: the systemd user unit and the launchd files, with install-time placeholders.
- `tests/`: every test, on `unittest`.
- `docs/`: `api.md`, `configuration.md`, `porting.md`.
- `deploy.toml`: how the repository is checked, tested and deployed.

## Rules

- The daemon runs on the Python standard library alone: no third-party package and no `bmo-*`
  package. `pyproject.toml` declares no dependencies, and a test enforces both.
- No Korean in code, comments, strings, tests, scripts, pages or deploy files; a test enforces it.
- Tests live in `tests/`. Check and test with:
  - `python3 -B -m py_compile src/*.py tests/*.py`
  - `python3 -B -m unittest discover -s tests`
- A change to any route, field or consumed contract in `docs/api.md` updates that document in the
  same change and bumps the major version in `pyproject.toml`. A new config key goes into
  `docs/configuration.md`.
- No personal paths, user names, user ids or host names in any file; the deploy files reach the
  home folder through `%h` or the `@HOME@` placeholder.
- No test touches a live instance's port or data directory: a test that needs the server starts one
  through `tests/siren_server.py`, on an ephemeral port with a temporary data directory.
