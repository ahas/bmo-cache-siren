#!/usr/bin/env python3
"""cache-siren CLI output formatter: stdin JSON → readable lines. usage: cli_fmt.py status|tail|ctl"""
import json, sys

mode = sys.argv[1] if len(sys.argv) > 1 else "status"
try:
    d = json.load(sys.stdin)
except Exception:
    print("❌ cache-siren daemon not responding")
    sys.exit(1)
if mode == "status":
    q = d.get("quota") or {}
    print(f"✅ cache-siren up {d.get('uptime_s')}s · files {d.get('files')} · events {d.get('events')} · last {d.get('last_event')} · 5h {q.get('five_h')}%")
elif mode == "tail":
    for e in d.get("events", []):
        print(f"{e.get('ts_kst',''):8} {e.get('project','')[:12]:12} {e.get('session',''):8} {e.get('m',''):9} {e.get('kind',''):7} "
              f"+{e.get('new',0):>8,} read={e.get('read',0):>9,} {str(e.get('note',''))[:14]:14} {str(e.get('req',''))[:60]}")
elif mode == "ctl":
    print("false" if d.get("visible", True) else "true")
