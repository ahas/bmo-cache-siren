#!/usr/bin/env python3
"""cache-siren — live watch of every Anthropic request: cache hit or new tokens.

Every request is recorded as a cache hit or as new tokens and shown in an always-on-top overlay;
a sudden rise in new tokens sounds a siren. The overlay names the session and the request.

Data = the assistant usage in Claude Code transcripts (per root in projects_globs: */*.jsonl, plus
subagents */*/subagents/*.jsonl).
  · There are several roots: each login has its own CLAUDE_CONFIG_DIR, so transcripts also pile up
    in account folders, not only under ~/.claude.
  · No new logging: only tails the lines the CLI already writes, polling every 1s (stat only on
    files with an mtime inside 6h).
  · 1 API request = 1 message.id (streaming writes several assistant lines) → dedupe by message.id.
  · Local (deepseek and the like) sessions bill nothing → recorded only, no siren (hidden by the
    overlay toggle by default).

Event kinds:
  warm    new tokens < yellow                                (normal cache hit)
  growth  cache_read>0 & new tokens ≥ yellow                 (a big Read, many tool_results — real cost)
  start   first request of a session (cache_read≈0, unseen)  (normal, new session prefill)
  cold    a seen session with cache_read==0 & new ≥ yellow   (re-prefill = TTL expiry, prefix change,
                                                              model switch, tool list change, compact)
  refusal system model_refusal_fallback (safeguard → automatic model switch = the next request
          re-prefills everything)
  quota   synthetic «hit your session limit»
  compact compact_boundary / summary
  error   isApiErrorMessage (other than quota)

Cause attribution (note, on cold/growth/start), surest first:
  session_start first request of a session · model_switch the model changed (refusal fallback
  included) · effort_model /model or /effort · compact a compact boundary · ttl_expired the gap
  since the previous request passed the TTL of that mode and model (the cache really died)
  prefix_changed the cache was alive yet the held context was not read = the prefix moved
                 (which of system prompt, documents in it, tool list or model line changed is not
                 in the transcript)
  tool_search · image only explain why new tokens rose (not why the prefix was lost) · unknown the rest

Alerts (siren wav + spoken text, tuned with --config):
  red   : cold ≥ red · growth ≥ red · refusal · 60s burst sum ≥ burst_red
  yellow: overlay colour only (no sound)
Threshold basis (1,851 transcript requests): p50 1.9K · p90 7.7K · p95 12K · p99 172K. ≥20K 3.3%
· ≥100K 1.6% = every real cold prefill of that day.

HTTP 127.0.0.1:<port>  (default 9022)
  GET  /            overlay page            GET /events?limit=N&since=SEQ   GET /stream (SSE)
  GET  /health      GET /stats              GET|POST /ctl  {visible,muted,opacity,show_local,page}
  POST /test-siren  (for checking — really sounds the siren and voice)
  GET|POST /ttl     {ttl_s, ttl_by_mode_model_s} (POST applies live and merges into the config file)
  POST /events      one request of a loopback runner that leaves no transcript
Every route, body and field is stated in docs/api.md; every config key in docs/configuration.md.

Log: ~/.claude/cache-siren/cache-siren.log (structured [ts] [cache-siren] [LEVEL] msg {json})
· canonical events events.jsonl · offsets state.json
"""
import argparse, datetime, glob, http.server, json, math, os, platform, re, socketserver, struct, subprocess, sys, threading, time, traceback, urllib.request, wave
from collections import deque, defaultdict

HOME = os.path.expanduser("~")
PROJ_DIR = os.path.join(HOME, ".claude", "projects")
# With several logins the CLI writes transcripts under each account's CLAUDE_CONFIG_DIR. So there
# is not one root to scan but several, and projects_globs in the config lists them (being globs,
# a newly added account is picked up as it is).
def proj_dirs():
    out = []
    for pat in CFG.get("projects_globs") or [PROJ_DIR]:
        for p in sorted(glob.glob(os.path.expanduser(pat))):
            if os.path.isdir(p) and p not in out:
                out.append(p)
    if not out:
        out.append(PROJ_DIR)
    return out


def proj_root_of(path):
    """Which root a transcript sits under. The default root when none matches."""
    for root in proj_dirs():
        if os.path.commonpath([os.path.abspath(path), root]) == root:
            return root
    return PROJ_DIR
RUN_DIR = os.path.join(HOME, ".claude", "cache-siren")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WEB_DIR = os.path.join(ROOT, "web")
LOG = os.path.join(RUN_DIR, "cache-siren.log")
EVENTS = os.path.join(RUN_DIR, "events.jsonl")
STATE = os.path.join(RUN_DIR, "state.json")
CTL = os.path.join(RUN_DIR, "ctl.json")
SIREN_WAV = os.path.join(RUN_DIR, "siren.wav")

DEFAULT_CONFIG = {
    "port": 9022,
    "poll_s": 1.0,
    # Transcript roots to scan. Each login has its own CLAUDE_CONFIG_DIR, so a deployment that keeps
    # account folders elsewhere adds their globs here.
    "projects_globs": [
        "~/.claude/projects",
    ],
    # Leading parts of an encoded project folder name (the cwd with "/" turned into "-") that are
    # cut off to give the short project name; the first match wins.
    "project_prefixes": [],
    "recent_hours": 6,            # tail only transcripts with an mtime inside this many hours
    "backfill_hours": 3,          # past span loaded into the overlay at start
    "yellow_new_tokens": 20000,
    "red_new_tokens": 100000,     # every real cold re-prefill observed sat above this
    "burst_window_s": 60,
    "burst_red_tokens": 500000,   # the size of many sessions resuming at once or a wide fan-out
    "siren_cooldown_s": 20,       # suppress repeated sirens for the same session
    "ttl_s": 3600,                # how long the prompt cache really lasts; a gap must pass it for ttl_expired
    # Within a mode the longest model prefix wins. * is that mode's default; only modes and models
    # missing here fall back to ttl_s above. A deployment may publish its own policy into this key.
    "ttl_by_mode_model_s": {
        "claude-code": {"*": 3600},
        "codex": {"gpt-5.6": 1800},
        "openrouter": {},
    },
    "prefix_reuse_min": 0.5,      # reading less than this share of the previous context means the prefix changed
    # Spoken alerts run voice_cmd with {text} replaced by the alert text; there is no default command.
    "voice": False,
    "voice_cmd": [],
    "sound": True,
    "siren_file": os.path.join(RUN_DIR, "warning.mp3"),   # played when present, else the generated siren.wav (afplay plays mp3 natively)
    "quota_url": "http://localhost:9200/api/claude",
    "heartbeat_url": "http://localhost:9200/api/services/heartbeat",
    "ring": 600,
    "retention_days": 7,          # lines of events.jsonl older than this are removed every 6h (0 = keep forever)
    "prune_every_s": 6 * 3600,
    "node": "",                   # node name sent with the heartbeat; empty = this host's name
}

RATES = {  # USD/Mtok (input, cache_write, cache_read), estimates
    "claude-fable-5-1": (10.0, 12.5, 0.25), "claude-fable-5": (10.0, 12.5, 0.25),
    "claude-opus-5": (5.0, 6.25, 0.5), "claude-sonnet-5": (3.0, 3.75, 0.3), "claude-haiku-4-5": (1.0, 1.25, 0.1),
    "claude-opus-4": (15.0, 18.75, 1.5), "claude-sonnet-4": (3.0, 3.75, 0.3),
}
# Model identifier → readable name. Every model name on screen comes from here: the table, the web
# page and the alert text all read this one list, so a model never shows under different names.
MODEL_NAMES = [("claude-fable-5-1", "Fable 5.1"), ("claude-fable-5", "Fable 5"), ("claude-opus-5", "Opus 5"),
               ("claude-opus-4-8", "Opus 4.8"), ("claude-opus-4", "Opus 4"), ("claude-sonnet-5", "Sonnet 5"),
               ("claude-sonnet-4", "Sonnet 4"), ("claude-haiku-4-5", "Haiku 4.5"), ("claude-haiku", "Haiku")]

CFG = dict(DEFAULT_CONFIG)
CONFIG_PATH = None                # the file the config was read from at start; POST /ttl merges into it
QUIET = False
_log_lock = threading.Lock()
_events_lock = threading.Lock()   # serialises the events.jsonl append (emit) and the prune rewrite
_config_lock = threading.Lock()   # serialises TTL updates, so the live table and the file agree on the last one

TTL_KEYS = ("ttl_s", "ttl_by_mode_model_s")


class TtlError(ValueError):
    """A TTL body that cannot be applied; args[0] is the English error code the caller gets."""


def load_config(path):
    """Read the config file once, at start, over the defaults. A missing file is created with the defaults."""
    global CONFIG_PATH
    CONFIG_PATH = path
    try:
        with open(path) as f:
            CFG.update(json.load(f))
    except FileNotFoundError:
        with open(path, "w") as f:
            json.dump(DEFAULT_CONFIG, f, ensure_ascii=False, indent=2)
        log("INFO", "config created with defaults", path=path)
    except Exception as e:
        log("ERROR", "config load failed — defaults", err=str(e))


def _positive_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def ttl_body(body):
    """Check a TTL body: one or both TTL keys, a positive integer of seconds for ttl_s and a
    {mode: {model prefix or "*": seconds}} table with non-empty string keys. An empty mode table
    is allowed: it says that provider has no uniform cache contract."""
    if not isinstance(body, dict):
        raise TtlError("body_not_object")
    if any(k not in TTL_KEYS for k in body):
        raise TtlError("unknown_field")
    if not body:
        raise TtlError("nothing_to_set")
    if "ttl_s" in body and not _positive_int(body["ttl_s"]):
        raise TtlError("ttl_s_not_positive_integer")
    if "ttl_by_mode_model_s" in body:
        table = body["ttl_by_mode_model_s"]
        if not isinstance(table, dict):
            raise TtlError("ttl_table_not_object")
        for mode, rows in table.items():
            if not isinstance(mode, str) or not mode or not isinstance(rows, dict):
                raise TtlError("ttl_table_mode_invalid")
            for prefix, seconds in rows.items():
                if not isinstance(prefix, str) or not prefix:
                    raise TtlError("ttl_table_model_invalid")
                if not _positive_int(seconds):
                    raise TtlError("ttl_table_seconds_not_positive_integer")
    return {k: body[k] for k in TTL_KEYS if k in body}


def persist_config(path, updates):
    """Merge only the given keys into the config file, by an atomic replace; every other key stays as written.
    A symlinked config is written through to its target, so the link stays a link."""
    path = os.path.realpath(path)
    try:
        with open(path) as f:
            current = json.load(f)
    except FileNotFoundError:
        current = {}
    if not isinstance(current, dict):
        raise ValueError("config file does not hold an object")
    current.update(updates)
    tmp = f"{path}.tmp-{os.getpid()}"
    try:
        with open(tmp, "w") as f:
            json.dump(current, f, ensure_ascii=False, indent=2)
            f.write("\n")
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def load_recent_events(path, limit):
    """Read only the last valid events of the durable log. One broken line does not wipe the rest of the ring."""
    rows = deque(maxlen=max(0, int(limit)))
    if rows.maxlen == 0:
        return []
    try:
        with open(path, "r") as f:
            for raw in f:
                try:
                    row = json.loads(raw)
                except Exception:
                    continue
                if isinstance(row, dict):
                    rows.append(row)
    except FileNotFoundError:
        return []
    return list(rows)


def now_local():
    return datetime.datetime.now().astimezone()


def log(level, msg, **kw):
    ts = now_local().strftime("%Y-%m-%dT%H:%M:%S")
    line = f"[{ts}] [cache-siren] [{level}] {msg}"
    if kw:
        line += " " + json.dumps(kw, ensure_ascii=False, default=str)
    with _log_lock:
        try:
            # Keep the log from growing without bound: past 8MB keep only the last 4MB (structured lines, so losing the head is harmless).
            if os.path.exists(LOG) and os.path.getsize(LOG) > 8_000_000:
                with open(LOG, "rb") as f:
                    f.seek(-4_000_000, os.SEEK_END)
                    tailbytes = f.read()
                cut = tailbytes.find(b"\n") + 1
                with open(LOG, "wb") as f:
                    f.write(b"[log truncated to last ~4MB]\n" + tailbytes[cut:])
            with open(LOG, "a") as f:
                f.write(line + "\n")
        except Exception:
            pass
    if not QUIET:
        print(line, flush=True)


def ts_local(iso):
    try:
        return datetime.datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone()
    except Exception:
        return None


def rate_for(model):
    m = (model or "").lower()
    if not m.startswith("claude"):
        return (0.0, 0.0, 0.0), "local"
    for k, v in RATES.items():
        if m.startswith(k):
            return v, "anthropic"
    return (10.0, 12.5, 0.25), "anthropic"


def model_name(model):
    m = model or ""
    for k, s in MODEL_NAMES:
        if m.startswith(k):
            return s
    if m == "<synthetic>":
        return "sys"
    return (m or "?")


def project_of(path):
    """Transcript path → (project, is_agent, parent_sid). Subagent = <proj>/<sid>/subagents/<agent>.jsonl"""
    rel = os.path.relpath(path, proj_root_of(path)).split(os.sep)
    proj = rel[0]
    for pre in CFG.get("project_prefixes") or ():
        if proj.startswith(pre):
            proj = proj[len(pre):]
            break
    if proj.startswith("-"):
        proj = proj.lstrip("-")
    is_agent = len(rel) >= 3 and rel[-2] == "subagents"
    parent = rel[1] if is_agent else None
    return proj, is_agent, parent


# ---------------------------------------------------------------- cause attribution
def ttl_for(mode, model, config=None):
    """The cache lifetime of one request. Within the mode's table the longest model prefix wins."""
    config = CFG if config is None else config
    by_mode = config.get("ttl_by_mode_model_s") or {}
    rows = by_mode.get(mode) if isinstance(by_mode, dict) else None

    if isinstance(rows, dict):
        named = [(prefix, seconds) for prefix, seconds in rows.items()
                 if prefix != "*" and isinstance(seconds, (int, float)) and (model or "").startswith(prefix)]
        if named:
            return max(named, key=lambda row: len(row[0]))[1]
        if isinstance(rows.get("*"), (int, float)):
            return rows["*"]

        # An explicitly listed empty mode means its provider has no uniform cache contract.
        # Falling through to the legacy global clock would invent one for OpenRouter.
        return None

    return config["ttl_s"]


def attribute(kind, flags, model, last_model, gap, read, prev_ctx, ttl_s=None, reuse_min=None, mode="claude-code"):
    """Pick the name of a re-prefill's cause (meaningful only for cold/growth/start).

    The sure things come first: a session's first request, a model switch, an effort switch and a
    compact are written into the transcript as they are. The clock comes next (TTL per mode and
    model): a gap past the cache lifetime means the cache really died. If the clock does not explain
    it and the held context was still not read, the prefix moved: when any of the system prompt,
    the documents in it, the tool list or the model line changes, the cache misses whatever the
    time. A request that read all it held and only added a lot on top does not belong here (that is
    just a big Read). The last two only explain why new tokens rose, not why the prefix was lost,
    so they come last.
    """
    ttl = ttl_for(mode, model) if ttl_s is None else ttl_s
    reuse = CFG["prefix_reuse_min"] if reuse_min is None else reuse_min
    if kind not in ("cold", "growth", "start"):
        return ""
    if kind == "start":
        return "session_start"
    if "refusal" in flags or (last_model and model != last_model):
        return "model_switch"
    if "effort_model" in flags:
        return "effort_model"
    if "compact" in flags:
        return "compact"
    if ttl is not None and gap is not None and gap > ttl:
        return "ttl_expired"
    if prev_ctx > 0 and read < prev_ctx * reuse:
        return "prefix_changed"
    if "tool_search" in flags:
        return "tool_search"
    if "image" in flags:
        return "image"
    return "unknown" if kind == "cold" else ""


# ---------------------------------------------------------------- siren wav
def ensure_siren_wav():
    if os.path.exists(SIREN_WAV):
        return
    sr = 22050
    dur = 2.6
    n = int(sr * dur)
    frames = bytearray()
    for i in range(n):
        t = i / sr
        f = 620 + 480 * (0.5 - 0.5 * math.cos(2 * math.pi * 1.6 * t))   # 620↔1100Hz sweep at 1.6Hz
        env = min(1.0, t / 0.05) * min(1.0, (dur - t) / 0.15)
        v = int(12000 * env * math.sin(2 * math.pi * f * t))
        frames += struct.pack("<h", v)
    with wave.open(SIREN_WAV, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr); w.writeframes(bytes(frames))
    log("INFO", "siren.wav generated", path=SIREN_WAV, seconds=dur)


# ---------------------------------------------------------------- state
class Session:
    __slots__ = ("seen", "last_ts", "last_model", "last_ctx", "flags", "last_req", "tool_names", "msg_ids", "agent_count")

    def __init__(self):
        self.seen = False
        self.last_ts = None
        self.last_model = None
        self.last_ctx = 0
        self.flags = set()
        self.last_req = ""
        self.tool_names = {}
        self.msg_ids = deque(maxlen=64)
        self.agent_count = 0


def burst_summary(recent):
    """Gather the requests in the 60-second window into one line. New tokens and cache reads are the
    real sums of the requests in the window, and the projects stand largest first: the first name
    owns the burst."""
    by = {}
    for _t, new, project, read in recent:
        name = project or "?"
        part = by.setdefault(name, {"project": name, "new": 0, "read": 0, "req": 0})
        part["new"] += new
        part["read"] += read
        part["req"] += 1
    parts = sorted(by.values(), key=lambda part: -part["new"])
    return {"new": sum(p["new"] for p in parts), "read": sum(p["read"] for p in parts),
            "req": sum(p["req"] for p in parts), "parts": parts}


def quota_from(payload, at):
    """Read the one slot the header shows from the quota source's JSON: the slot the source marks as
    supplying now. Order of choice: the slot with `active: true` → the slot `usage.active.account`
    names → slot "1" → `real_*`."""
    u = payload.get("usage") or {}
    active = u.get("active") or {}
    slots = u.get("quota_slots") or {}
    ordered = [slots[key] for key in sorted(slots, key=lambda key: int(key) if str(key).isdigit() else 0)]
    slot = (next((s for s in ordered if s.get("active") is True), None)
            or next((s for s in ordered if "account" in active and s.get("account") == active.get("account")), None)
            or slots.get("1") or {})
    return {"five_h": slot.get("five_hour_utilization", u.get("real_5h_pct")),
            "five_h_resets": slot.get("five_hour_resets_at"),
            "seven_d": slot.get("seven_day_utilization", u.get("real_week_pct")),
            "seven_d_resets": slot.get("seven_day_resets_at"),
            "tier": slot.get("tier", active.get("name", "")),
            "account": slot.get("account", active.get("account")),
            "provider": slot.get("provider", active.get("provider")),
            "accounts": active.get("accounts"),
            "at": at}


class Engine:
    def __init__(self):
        self.sessions = {}          # sid -> Session
        self.offsets = {}           # path -> offset
        self.ring = deque(maxlen=CFG["ring"])
        self.seq = 0
        self.cond = threading.Condition()
        self.recent = deque()       # (request epoch, new, project, read) for the 60s burst
        self.last_siren = {}        # sid -> epoch
        self.last_burst_siren = 0.0
        self.ctl = {"visible": True, "muted": False, "opacity": 0.94, "show_local": False, "page": "/"}
        self.quota = {}
        # Quota conversion (estimate): tokens observed in the current 5h window against the rise of the
        # 5h % → a %/Mtok slope. On a flat-rate plan $ means nothing, so each event converts to "% of
        # the 5h quota". Estimated from what is observed (no official figure).
        # 🔴 The 5h limit is a rolling (sliding) 5-hour window: usage older than 5h drops out and the %
        # falls (measured 62%→32% with no reset). So a fixed window (reset to now) is wrong; tokens of
        # the last 5 hours are summed in per-minute buckets.
        self.minute = {}   # minute_epoch(int) -> {"total": n, "by": {project: n}}
        self.started = time.time()
        self.files_watched = 0
        self.last_event_ts = None
        self._load()

    # ---- persistence
    def _load(self):
        try:
            st = json.load(open(STATE))
            self.offsets = st.get("offsets", {})
        except Exception:
            self.offsets = {}
        try:
            self.ctl.update(json.load(open(CTL)))
        except Exception:
            pass
        # Offsets may already point at the end of every transcript after a restart. Restore the
        # visible ring from its own durable log first, then let genuinely new transcript rows
        # append to it. Sequence numbers belong to this process and are assigned afresh.
        for ev in load_recent_events(EVENTS, CFG["ring"]):
            self.seq += 1
            ev["seq"] = self.seq
            self.ring.append(ev)
        if self.ring:
            self.last_event_ts = self.ring[-1].get("ts_kst")

    def save(self):
        try:
            tmp = STATE + ".tmp"
            json.dump({"offsets": self.offsets}, open(tmp, "w"))
            os.replace(tmp, STATE)
        except Exception as e:
            log("ERROR", "state save failed", err=str(e))

    def save_ctl(self):
        try:
            json.dump(self.ctl, open(CTL, "w"))
        except Exception:
            pass

    # ---- scanning
    def files(self):
        horizon = time.time() - CFG["recent_hours"] * 3600
        out = []
        for root in proj_dirs():
            for pat in ("*/*.jsonl", "*/*/subagents/*.jsonl"):
                for p in glob.glob(os.path.join(root, pat)):
                    try:
                        if os.path.getmtime(p) >= horizon:
                            out.append(p)
                    except OSError:
                        pass
        return out

    def backfill(self):
        """At start: files inside backfill_hours are read from the beginning (emitting only events inside
        the window, silently); the rest are skipped to their end."""
        horizon = time.time() - CFG["backfill_hours"] * 3600
        n = 0
        for p in self.files():
            if p in self.offsets:
                continue
            if os.path.getmtime(p) >= horizon:
                self.offsets[p] = 0
                n += self.scan(p, quiet=True, min_epoch=horizon)
            else:
                self.offsets[p] = os.path.getsize(p)
        # Read file by file, so sort by time and renumber seq (the overlay draws in seq order)
        with self.cond:
            evs = sorted(self.ring, key=lambda e: e.get("epoch", 0))
            self.ring.clear()
            self.seq = 0
            for e in evs:
                self.seq += 1
                e["seq"] = self.seq
                self.ring.append(e)
        self.save()
        log("INFO", "backfill done", events=n, files=len(self.offsets))

    def scan(self, path, quiet=False, min_epoch=0.0):
        off = self.offsets.get(path, 0)
        try:
            size = os.path.getsize(path)
        except OSError:
            return 0
        if size < off:            # file truncated or rewritten
            off = 0
        if size == off:
            return 0
        with open(path, "rb") as f:
            f.seek(off)
            data = f.read()
        if not data.endswith(b"\n"):
            cut = data.rfind(b"\n") + 1
            data, tail = data[:cut], data[cut:]
        else:
            tail = b""
        self.offsets[path] = off + len(data)
        proj, is_agent, parent = project_of(path)
        sid = os.path.basename(path)[:-6]
        n = 0
        for raw in data.split(b"\n"):
            if not raw:
                continue
            try:
                d = json.loads(raw)
            except Exception:
                continue
            try:
                ev = self.handle(d, sid, proj, is_agent, parent)
            except Exception:
                log("ERROR", "handle failed", path=path, tb=traceback.format_exc()[-800:])
                continue
            if ev:
                t = ev.get("epoch", 0)
                if t >= min_epoch:
                    self.emit(ev, quiet=quiet)
                    n += 1
        return n

    # ---- per-line
    def handle(self, d, sid, proj, is_agent, parent):
        typ = d.get("type")
        s = self.sessions.get(sid)
        if s is None:
            s = self.sessions[sid] = Session()
        ts = d.get("timestamp")
        if typ == "user":
            m = d.get("message") or {}
            c = m.get("content")
            if isinstance(c, str):
                txt = c.strip()
                if txt.startswith("<local-command") or txt.startswith("<command-name>"):
                    if "/model" in txt or "/effort" in txt:
                        s.flags.add("effort_model")
                    s.last_req = "⌘ " + re.sub(r"<[^>]+>", " ", txt).strip()[:70]
                elif d.get("isMeta") and "cross-session" in txt:
                    body = re.sub(r"<[^>]+>", " ", txt).strip()
                    s.last_req = "✉ " + " ".join(body.split())[:70]
                elif txt.startswith("[Request interrupted"):
                    s.last_req = "⏹ interrupted"
                else:
                    s.last_req = "⌨ " + " ".join(re.sub(r"<[^>]+>", " ", txt).split())[:70]
            elif isinstance(c, list):
                names = []
                has_img = False
                text = ""
                for x in c:
                    if not isinstance(x, dict):
                        continue
                    if x.get("type") == "tool_result":
                        names.append(s.tool_names.get(x.get("tool_use_id"), "tool"))
                    elif x.get("type") == "image":
                        has_img = True
                    elif x.get("type") == "text" and not text:
                        text = x.get("text", "")
                if has_img:
                    s.flags.add("image")
                if names:
                    s.last_req = "⚙ " + ",".join(names[:3]) + (f" +{len(names)-3}" if len(names) > 3 else "")
                elif text:
                    tt = " ".join(re.sub(r"<[^>]+>", " ", text).split())
                    s.last_req = ("✉ " if d.get("isMeta") else "⌨ ") + tt[:70]
            return None
        if typ == "system":
            sub = d.get("subtype")
            if sub == "model_refusal_fallback":
                s.flags.add("refusal")
                return self.mk(d, sid, proj, is_agent, parent, s, kind="refusal", model=d.get("originalModel"),
                               note=f"{d.get('apiRefusalCategory')} → {model_name(d.get('fallbackModel'))} (scope {d.get('scope')})",
                               new=0, read=0, out=0, inp=0)
            if sub == "local_command":
                c = str(d.get("content", ""))
                if "/model" in c or "/effort" in c:
                    s.flags.add("effort_model")
                    if "Set model" in c or "effort" in c.lower():
                        return self.mk(d, sid, proj, is_agent, parent, s, kind="cmd", model=s.last_model,
                                       note=re.sub(r"<[^>]+>", " ", c).strip()[:90], new=0, read=0, out=0, inp=0)
            if sub in ("compact_boundary",):
                s.flags.add("compact")
                return self.mk(d, sid, proj, is_agent, parent, s, kind="compact", model=s.last_model, note="compact", new=0, read=0, out=0, inp=0)
            return None
        if typ == "summary":
            s.flags.add("compact")
            return None
        if typ != "assistant":
            return None
        m = d.get("message") or {}
        mid = m.get("id")
        model = m.get("model") or ""
        content = m.get("content")
        if isinstance(content, list):
            for x in content:
                if isinstance(x, dict) and x.get("type") == "tool_use":
                    s.tool_names[x.get("id")] = x.get("name", "tool")
                    if x.get("name") == "ToolSearch":
                        s.flags.add("tool_search")
            if len(s.tool_names) > 400:
                for k in list(s.tool_names)[:200]:
                    s.tool_names.pop(k, None)
        if d.get("isApiErrorMessage") or model == "<synthetic>":
            txt = ""
            if isinstance(content, list):
                txt = " ".join(x.get("text", "") for x in content if isinstance(x, dict) and x.get("type") == "text")
            elif isinstance(content, str):
                txt = content
            # Only the isApiErrorMessage mark the CLI adds says failure. A <synthetic> line without it
            # is not a model failure but a notice the CLI leaves, from a turn with no answer to give
            # (a compact, say), so it stands as a notice, not an error, and raises no alert or log.
            if d.get("isApiErrorMessage"):
                # A limit is a wall, not a failure. The CLI words its limits by window (session), week
                # (weekly) and month (monthly spend), all in the shape "hit your … limit", so the shape
                # is matched rather than one more word. An older CLI's usage limit stands in the same place.
                low = txt.lower()
                limit = ("session limit" in low or "usage limit" in low
                         or ("hit your" in low and "limit" in low))
                kind = "quota" if limit else "error"
            else:
                kind = "notice"
            key = ("err", ts, txt[:40])
            if key in s.msg_ids:
                return None
            s.msg_ids.append(key)
            return self.mk(d, sid, proj, is_agent, parent, s, kind=kind, model=s.last_model or model, note=txt[:90], new=0, read=0, out=0, inp=0)
        u = m.get("usage")
        if not u or not mid:
            return None
        if mid in s.msg_ids:
            return None
        s.msg_ids.append(mid)
        inp = int(u.get("input_tokens") or 0)
        cc = int(u.get("cache_creation_input_tokens") or 0)
        cr = int(u.get("cache_read_input_tokens") or 0)
        out = int(u.get("output_tokens") or 0)
        new = inp + cc
        ctx = new + cr
        backend = rate_for(model)[1]
        t = ts_local(ts) if ts else None
        gap = (t - s.last_ts).total_seconds() if (t and s.last_ts) else None
        # kind
        if not s.seen:
            kind = "start"
        elif cr == 0 and new >= CFG["yellow_new_tokens"]:
            kind = "cold"
        elif new >= CFG["yellow_new_tokens"]:
            kind = "growth"
        else:
            kind = "warm"
        attr = attribute(kind, s.flags, model, s.last_model, gap, cr, s.last_ctx)
        ev = self.mk(d, sid, proj, is_agent, parent, s, kind=kind, model=model, note=attr, new=new, read=cr, out=out, inp=inp, cc=cc, gap=gap, prev_ctx=s.last_ctx)
        s.seen = True
        s.last_ts = t or s.last_ts
        s.last_model = model
        s.last_ctx = ctx
        s.flags.clear()
        return ev

    def mk(self, d, sid, proj, is_agent, parent, s, kind, model, note, new, read, out, inp, cc=None, gap=None, prev_ctx=0):
        ts = d.get("timestamp") or ""
        t = ts_local(ts) or now_local()
        r, backend = rate_for(model)
        cost = 0.0
        if cc is not None:
            cost = (inp * r[0] + cc * r[1] + read * r[2]) / 1e6
        level = "info"
        if kind in ("cold", "growth") and new >= CFG["red_new_tokens"]:
            level = "red"
        elif kind == "refusal":
            level = "red"
        elif kind in ("cold", "growth", "start") and new >= CFG["yellow_new_tokens"]:
            level = "yellow"
        elif kind in ("quota", "error", "cmd", "compact"):
            level = "yellow" if kind != "cmd" else "info"
        if backend == "local" and level == "red":
            level = "yellow"   # local bills nothing, so no sound
        return {
            "ts_kst": t.strftime("%H:%M:%S"), "date_kst": t.strftime("%m-%d"), "epoch": t.timestamp(),
            "project": proj, "session": sid[:8], "sid": sid, "agent": is_agent, "parent": (parent or "")[:8],
            "cwd": os.path.basename(d.get("cwd") or ""),
            "mode": "claude-code", "model": model or "", "m": model_name(model), "backend": backend,
            "kind": kind, "level": level, "note": note or "",
            "new": new, "input": inp, "cc": (cc or 0), "read": read, "out": out, "ctx": new + read, "prev_ctx": prev_ctx,
            "gap_s": None if gap is None else int(gap), "cost": round(cost, 3),
            "req": s.last_req or "",
        }

    def external(self, body):
        """Turn one request of a loopback runner that leaves no transcript into the common event."""
        if not isinstance(body, dict):
            raise ValueError("body must be an object")
        mode = body.get("mode")
        if mode not in ("codex", "openrouter"):
            raise ValueError("mode must be codex or openrouter")

        def word(name, required=False):
            value = body.get(name, "")
            if not isinstance(value, str) or len(value) > 500 or (required and not value.strip()):
                raise ValueError(f"{name} must be a short string")
            return value.strip()

        def count(name):
            value = body.get(name, 0)
            if isinstance(value, bool) or not isinstance(value, (int, float)) or value < 0:
                raise ValueError(f"{name} must be a non-negative number")
            return int(value)

        model = word("model", required=True)
        key = word("key", required=True)
        agent = word("agent")
        project = word("project") or key
        cwd = word("cwd")
        sid = word("session")
        inp = count("input_tokens")
        cc = count("cache_write_input_tokens")
        read = count("cache_read_input_tokens")
        out = count("output_tokens")
        known = body.get("usage_known") is True
        new = inp + cc
        # Transcript scanning and loopback ingestion run on different HTTP/worker threads. Take a
        # stable snapshot under the same condition lock emit() uses so a concurrent append cannot
        # invalidate deque iteration while this request is classified.
        with self.cond:
            recent = list(self.ring)
        seen = bool(sid) and any(
            ev.get("mode") == mode and ev.get("sid") == sid and ev.get("model") == model
            for ev in recent
        )

        if not known:
            kind, note, level = "request", "usage_unavailable", "info"
        elif read > 0 and new >= CFG["yellow_new_tokens"]:
            kind, note = "growth", ""
            level = "red" if new >= CFG["red_new_tokens"] else "yellow"
        elif read > 0:
            kind, note, level = "warm", "", "info"
        elif seen and new >= CFG["yellow_new_tokens"]:
            kind, note = "cold", "unknown"
            level = "red" if new >= CFG["red_new_tokens"] else "yellow"
        else:
            kind, note = "start" if not seen else "request", "session_start" if not seen else ""
            level = "yellow" if new >= CFG["yellow_new_tokens"] else "info"

        t = now_local()
        return {
            "ts_kst": t.strftime("%H:%M:%S"), "date_kst": t.strftime("%m-%d"), "epoch": t.timestamp(),
            "project": project, "session": sid[:8], "sid": sid, "agent": agent, "parent": "", "cwd": cwd,
            "mode": mode, "model": model, "m": model_name(model),
            "backend": "openai" if mode == "codex" else "openrouter",
            "kind": kind, "level": level, "note": note,
            "new": new, "input": inp, "cc": cc, "read": read, "out": out,
            "ctx": new + read, "prev_ctx": 0, "gap_s": None, "cost": 0, "req": "",
        }

    # ---- emit / alerts
    def emit(self, ev, quiet=False):
        with self.cond:
            self.seq += 1
            ev["seq"] = self.seq
            self.ring.append(ev)
            self.last_event_ts = ev["ts_kst"]
            self.cond.notify_all()
        try:
            with _events_lock:
                with open(EVENTS, "a") as f:
                    f.write(json.dumps(ev, ensure_ascii=False) + "\n")
        except Exception as e:
            log("ERROR", "events append failed", err=str(e))
        if ev["level"] in ("yellow", "red") or ev["kind"] in ("refusal", "quota", "cmd", "compact"):
            log("WARN" if ev["level"] == "red" else "INFO",
                f"{ev['kind'].upper()} {ev['project']}/{ev['session']} {ev['m']} +{ev['new']:,} read={ev['read']:,} {ev['note']} «{ev['req']}»",
                lvl=ev["level"], cost=ev["cost"], agent=ev["agent"])
        if quiet:
            return
        now = time.time()
        if ev.get("backend") != "local" and ev.get("new"):
            # The window is measured by when the request went out. Measured by when the watcher read
            # the line, a transcript written late (a finished subagent flushing at once) would pack
            # requests from different minutes into one 60 seconds.
            self.recent.append((ev.get("epoch") or now, ev["new"], ev["project"], ev.get("read") or 0))
        if ev.get("backend") == "anthropic" and ev.get("new"):
            self._add_bucket(ev.get("epoch") or now, ev["project"] or "?", ev["new"])
        self.recent = deque(x for x in self.recent if x[0] >= now - CFG["burst_window_s"])
        if ev["level"] == "red":
            self.alert(ev)
        burst = burst_summary(self.recent)
        if burst["new"] >= CFG["burst_red_tokens"] and now - self.last_burst_siren > CFG["burst_window_s"]:
            self.last_burst_siren = now
            projs = ",".join(part["project"] for part in burst["parts"][:4])
            self.alert({"kind": "burst", "project": projs, "session": "", "new": burst["new"], "level": "red",
                        "note": f"{burst['req']} req/{CFG['burst_window_s']}s", "m": ""}, force=True)
            with self.cond:
                self.seq += 1
                t = now_local()
                bev = {"seq": self.seq, "ts_kst": t.strftime("%H:%M:%S"), "date_kst": t.strftime("%m-%d"), "epoch": now,
                       "kind": "burst", "level": "red", "project": projs, "session": "", "m": "",
                       "new": burst["new"], "read": burst["read"], "note": f"{burst['req']} req in {CFG['burst_window_s']}s",
                       "parts": burst["parts"], "req": "", "backend": "anthropic", "cost": 0}
                self.ring.append(bev)
                self.cond.notify_all()

    def alert(self, ev, force=False):
        now = time.time()
        key = ev.get("sid") or ev.get("kind")
        if not force and now - self.last_siren.get(key, 0) < CFG["siren_cooldown_s"]:
            return
        self.last_siren[key] = now
        muted = self.ctl.get("muted")
        log("WARN", f"SIREN {ev['kind']} {ev.get('project')} +{ev.get('new', 0):,} muted={muted}")
        if muted:
            return
        if CFG["sound"]:
            snd = CFG.get("siren_file") or ""
            if not (snd and os.path.exists(snd)):
                snd = SIREN_WAV
            try:
                subprocess.Popen(["/usr/bin/afplay", snd], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception as e:
                log("ERROR", "afplay failed", err=str(e), file=snd)
        if CFG["voice"] and CFG.get("voice_cmd"):
            k = ev["kind"]
            if k == "refusal":
                text = f"Session {ev.get('project')}: a safeguard refusal switched the model. The next request re-prefills everything."
            elif k == "burst":
                text = f"Burst: {ev['new']} new tokens within one minute. {ev.get('project')}."
            elif k == "cold":
                text = f"Session {ev.get('project')} cold prefill, {ev['new']} new tokens. Cause {ev.get('note') or 'unknown'}."
            else:
                text = f"Session {ev.get('project')} grew by {ev['new']} new tokens."
            try:
                cmd = [a.replace("{text}", text) for a in CFG["voice_cmd"]]
                subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception as e:
                log("ERROR", "voice failed", err=str(e))

    # ---- cache lifetime
    def ttl(self):
        with self.cond:
            return {k: CFG[k] for k in TTL_KEYS}

    def set_ttl(self, updates):
        """Apply a checked TTL body live (attribution reads CFG at each request), then merge it into the
        config file so a restart keeps it. Returns whether the file was written."""
        with _config_lock:
            with self.cond:
                before = {k: CFG[k] for k in TTL_KEYS}
                CFG.update(updates)
            log("INFO", "ttl set", before=before, after={k: CFG[k] for k in TTL_KEYS}, config=CONFIG_PATH)
            if not CONFIG_PATH:
                return False
            persist_config(CONFIG_PATH, updates)
            return True

    # ---- stats
    def stats(self):
        now = time.time()
        per_min = [0] * 10
        for ev in list(self.ring):
            if ev.get("backend") == "local" or not ev.get("new"):
                continue
            age = now - ev.get("epoch", now)
            if 0 <= age < 600:
                per_min[9 - int(age // 60)] += ev["new"]
        live = {ev["sid"] for ev in self.ring if ev.get("backend") != "local" and now - ev.get("epoch", 0) < 1800 and ev.get("sid")}
        # Rolling 5h: %/Mtok estimated from the tokens observed over the last 5 hours, plus the budget bar
        # (the 5h limit is a sliding window, so no fixed window is assumed).
        util = self.quota.get("five_h")
        tot, by = self._rolling5h()
        # %/Mtok estimate = current 5h % / tokens observed over the last 5h (Mtok). Only what is observed
        # counts, so it is a bound (real use ≥ observed → real %/Mtok ≤ this value).
        rate = round(util / (tot / 1e6), 3) if (util and tot > 0) else None
        q = dict(self.quota)
        q["pct_per_mtok"] = rate
        q["window_tokens"] = tot
        # Budget bar (relative): split the 5h utilisation (util %) by each project's share of the tokens
        # observed over the last 5 hours. The rest = headroom.
        segs = []
        if util is not None and tot > 0:
            for p, t in sorted(by.items(), key=lambda x: -x[1]):
                segs.append({"project": p, "tokens": t, "pct": round(t / tot * util, 2)})
        q["budget"] = {"used_pct": util, "headroom_pct": (None if util is None else round(100 - util, 1)),
                       "segments": segs[:12], "rolling_5h": True}
        return {"per_min": per_min, "new_60s": sum(x[1] for x in self.recent if x[0] > now - 60), "sessions_30m": len(live),
                "quota": q, "uptime_s": int(now - self.started), "files": self.files_watched, "last_event": self.last_event_ts,
                "ctl": self.ctl, "cfg": {k: CFG[k] for k in ("yellow_new_tokens", "red_new_tokens", "burst_red_tokens")}}

    # ---- loops
    def loop(self):
        n = 0
        while True:
            try:
                fs = self.files()
                self.files_watched = len(fs)
                for p in fs:
                    self.scan(p)
                n += 1
                if n % 30 == 0:
                    self.save()
            except Exception:
                log("ERROR", "scan loop", tb=traceback.format_exc()[-800:])
            time.sleep(CFG["poll_s"])

    def prune_loop(self):
        # First run 30s after start (apart from the start-up rush), then every prune_every_s
        time.sleep(30)
        while True:
            try:
                self.prune_events()
            except Exception:
                log("ERROR", "prune failed", tb=traceback.format_exc()[-800:])
            time.sleep(max(600, CFG.get("prune_every_s", 6 * 3600)))

    def prune_events(self):
        """Remove lines older than retention_days from events.jsonl (atomic rewrite), serialised with the append by _events_lock."""
        days = CFG.get("retention_days", 7)
        if not days or days <= 0 or not os.path.exists(EVENTS):
            return
        horizon = time.time() - days * 86400
        kept = []
        removed = 0
        with _events_lock:
            with open(EVENTS, "r") as f:
                for line in f:
                    if not line.strip():
                        continue
                    try:
                        ep = json.loads(line).get("epoch", 0) or 0
                    except Exception:
                        ep = 0
                    if ep >= horizon:
                        kept.append(line if line.endswith("\n") else line + "\n")
                    else:
                        removed += 1
            if removed:
                tmp = EVENTS + ".tmp"
                with open(tmp, "w") as f:
                    f.writelines(kept)
                os.replace(tmp, EVENTS)
        if removed:
            log("INFO", "events pruned", removed=removed, kept=len(kept), retention_days=days)

    def _add_bucket(self, epoch, project, n):
        m = int(epoch // 60)
        b = self.minute.get(m)
        if b is None:
            b = self.minute[m] = {"total": 0, "by": defaultdict(int)}
        b["total"] += n
        b["by"][project] += n

    def _rolling5h(self):
        """Tokens observed over the last 5 hours (rolling), in total and per project. Buckets older than 5h are evicted."""
        cutoff = int((time.time() - 5 * 3600) // 60)
        for m in [m for m in self.minute if m < cutoff]:
            del self.minute[m]
        tot = 0
        by = defaultdict(int)
        for b in self.minute.values():
            tot += b["total"]
            for p, n in b["by"].items():
                by[p] += n
        return tot, by

    def seed_buckets(self):
        """At start, restore the minute buckets from the last 5 hours of events.jsonl (fills the rolling window at once)."""
        cutoff = time.time() - 5 * 3600
        n = 0
        try:
            with open(EVENTS) as f:
                for line in f:
                    if '"anthropic"' not in line:
                        continue
                    try:
                        d = json.loads(line)
                    except Exception:
                        continue
                    if d.get("backend") == "anthropic" and (d.get("epoch") or 0) >= cutoff:
                        self._add_bucket(d["epoch"], d.get("project") or "?", d.get("new", 0) or 0)
                        n += 1
        except FileNotFoundError:
            pass
        tot, _ = self._rolling5h()
        log("INFO", "rolling 5h buckets seeded from events.jsonl", events=n, window_tokens=tot)

    def quota_loop(self):
        while True:
            # Where there is no quota source it is not asked at all: the header % just stays "–", and
            # the log is not filled with a failure warning every 30 seconds.
            if not CFG.get("quota_url"):
                time.sleep(30)
                continue
            try:
                with urllib.request.urlopen(CFG["quota_url"], timeout=5) as r:
                    d = json.load(r)
                self.quota = quota_from(d, now_local().strftime("%H:%M"))
            except Exception as e:
                log("WARN", "quota fetch failed", err=str(e)[:120])
            time.sleep(30)

    def heartbeat_loop(self):
        port = CFG["port"]
        _last_hb_log = 0.0   # one INFO line every 10 minutes even with no requests, so a log-staleness watcher does not misfire
        while True:
            if time.time() - _last_hb_log >= 600:
                _last_hb_log = time.time()
                try:
                    log("INFO", "heartbeat alive", events=len(self.ring), files=self.files_watched, last_event=self.last_event_ts, uptime_s=int(time.time() - self.started))
                except Exception as e:
                    log("WARN", "heartbeat log failed", err=str(e)[:120])
            # A service registry, if the deployment has one. Without it only the local log line is written.
            if not CFG.get("heartbeat_url"):
                time.sleep(45)
                continue
            try:
                body = json.dumps({"name": "cache-siren", "node": CFG.get("node") or platform.node(), "port": port, "kind": "service", "engine": "monitor",
                                   "health_url": f"http://127.0.0.1:{port}/health", "endpoint": f"http://127.0.0.1:{port}/",
                                   "status": "up", "capabilities": {"notes": "live per-request cache/new-token watch + siren for Anthropic requests (transcript tail)"}}).encode()
                req = urllib.request.Request(CFG["heartbeat_url"], data=body, headers={"Content-Type": "application/json"})
                urllib.request.urlopen(req, timeout=5).read()
            except Exception as e:
                log("WARN", "heartbeat failed", err=str(e)[:120])
            time.sleep(45)


ENGINE = None


# ---------------------------------------------------------------- http
class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "cache-siren/1.0"

    def log_message(self, fmt, *args):  # access logging is debug-level; off to keep stream polling quiet
        pass

    def _serve_file(self, fpath, ctype):
        try:
            body = open(fpath, "rb").read()
        except Exception as e:
            return self._json(500, {"ok": False, "error": str(e), "file": fpath})
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path, _, qs = self.path.partition("?")
        q = dict(x.split("=", 1) for x in qs.split("&") if "=" in x)
        if path == "/" or path == "/index.html":
            return self._serve_file(os.path.join(WEB_DIR, "index.html"), "text/html; charset=utf-8")
        if path == "/demo":
            return self._serve_file(os.path.join(WEB_DIR, "demo.html"), "text/html; charset=utf-8")
        if path.startswith("/asset/"):
            name = os.path.basename(path)
            if name in ("siren_on.png", "siren_off.png"):
                return self._serve_file(os.path.join(RUN_DIR, name), "image/png")
            return self._json(404, {"ok": False, "error": "asset not allowed"})
        if path == "/health":
            st = ENGINE.stats()
            return self._json(200, {"ok": True, "service": "cache-siren", "uptime_s": st["uptime_s"], "files": st["files"],
                                    "last_event": st["last_event"], "events": len(ENGINE.ring), "quota": st["quota"]})
        if path == "/stats":
            return self._json(200, ENGINE.stats())
        if path == "/ctl":
            return self._json(200, ENGINE.ctl)
        if path == "/ttl":
            return self._json(200, ENGINE.ttl())
        if path == "/events":
            lim = int(q.get("limit", "80"))
            since = int(q.get("since", "0"))
            show_local = q.get("local", "0") == "1"
            evs = [e for e in list(ENGINE.ring) if e["seq"] > since and (show_local or e.get("backend") != "local")]
            return self._json(200, {"events": evs[-lim:], "seq": ENGINE.seq})
        if path == "/stream":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Connection", "keep-alive")
            self.end_headers()
            since = int(q.get("since", str(ENGINE.seq)))
            last_stats = 0.0
            try:
                while True:
                    with ENGINE.cond:
                        ENGINE.cond.wait(timeout=1.0)
                        pend = [e for e in list(ENGINE.ring) if e["seq"] > since]
                    for e in pend:
                        since = e["seq"]
                        self.wfile.write(f"event: ev\ndata: {json.dumps(e, ensure_ascii=False)}\n\n".encode())
                    if time.time() - last_stats > 2.0:
                        last_stats = time.time()
                        self.wfile.write(f"event: stats\ndata: {json.dumps(ENGINE.stats(), ensure_ascii=False)}\n\n".encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                return
        return self._json(404, {"ok": False, "error": "not found"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n > 65536:
            return self._json(413, {"ok": False, "error": "body too large"})
        raw = self.rfile.read(n) if n else b""
        parsed = True
        try:
            body = json.loads(raw or b"{}")
        except Exception:
            body, parsed = {}, False
        path = self.path.split("?")[0]
        if path == "/ttl":
            if not parsed:
                return self._json(400, {"ok": False, "error": "body_not_json"})
            try:
                updates = ttl_body(body)
            except TtlError as e:
                return self._json(400, {"ok": False, "error": e.args[0]})
            try:
                persisted = ENGINE.set_ttl(updates)
            except Exception as e:
                log("ERROR", "ttl persist failed", err=str(e), config=CONFIG_PATH)
                return self._json(500, {"ok": False, "error": "config_write_failed", "applied": True, **ENGINE.ttl()})
            return self._json(200, {"ok": True, "persisted": persisted, **ENGINE.ttl()})
        if path == "/events":
            try:
                ev = ENGINE.external(body)
            except ValueError as e:
                return self._json(400, {"ok": False, "error": str(e)})
            ENGINE.emit(ev)
            return self._json(200, {"ok": True, "seq": ev["seq"]})
        if path == "/ctl":
            before = dict(ENGINE.ctl)
            for k in ("visible", "muted", "opacity", "show_local", "page"):
                if k in body:
                    ENGINE.ctl[k] = body[k]
            ENGINE.save_ctl()
            log("INFO", "ctl", before=before, after=ENGINE.ctl)
            return self._json(200, ENGINE.ctl)
        if path == "/test-siren":
            ENGINE.alert({"kind": "cold", "project": "test", "sid": "test", "new": 123456, "level": "red", "note": "manual test", "m": ""}, force=True)
            return self._json(200, {"ok": True, "muted": ENGINE.ctl.get("muted")})
        return self._json(404, {"ok": False})


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    global ENGINE, QUIET
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=os.path.join(ROOT, "config.json"))
    ap.add_argument("--quiet", action="store_true")
    ap.add_argument("--no-backfill", action="store_true")
    a = ap.parse_args()
    QUIET = a.quiet
    os.makedirs(RUN_DIR, exist_ok=True)
    load_config(a.config)
    log("INFO", "start", pid=os.getpid(), port=CFG["port"], cfg={k: CFG[k] for k in ("yellow_new_tokens", "red_new_tokens", "burst_red_tokens", "poll_s")})
    ensure_siren_wav()
    ENGINE = Engine()
    if not a.no_backfill:
        ENGINE.backfill()
    else:
        for p in ENGINE.files():
            ENGINE.offsets.setdefault(p, os.path.getsize(p))
    ENGINE.seed_buckets()   # fill the rolling 5h window from events.jsonl at once
    for fn in (ENGINE.loop, ENGINE.quota_loop, ENGINE.heartbeat_loop, ENGINE.prune_loop):
        threading.Thread(target=fn, daemon=True).start()
    srv = Server(("127.0.0.1", CFG["port"]), Handler)
    log("INFO", "listening", addr=f"127.0.0.1:{CFG['port']}")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        ENGINE.save()
        log("INFO", "stop")


if __name__ == "__main__":
    main()
