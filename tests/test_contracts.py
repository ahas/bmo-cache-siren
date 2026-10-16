"""The HTTP contracts other services hold this daemon to.

The collection caller posts one event per request of a runner that leaves no transcript, in the
twelve field names of its Event struct. The console reader asks GET /stats and GET /events?limit=N
and reads the fields listed here. docs/api.md states both; a change to either is a major version."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from siren_server import SirenServer, cache_siren

# The collection caller's Event struct, field for field.
EVENT_FIELDS = ("mode", "model", "key", "agent", "project", "cwd", "session",
                "input_tokens", "cache_write_input_tokens", "cache_read_input_tokens", "output_tokens", "usage_known")

# What the console reader reads from GET /stats, and from each row of GET /events.
STATS_FIELDS = ("per_min", "new_60s", "sessions_30m", "uptime_s", "files", "last_event", "cfg", "quota")
STATS_CFG_FIELDS = ("yellow_new_tokens", "red_new_tokens", "burst_red_tokens")
STATS_QUOTA_FIELDS = ("five_h", "five_h_resets", "seven_d", "seven_d_resets", "tier", "provider", "accounts")
ROW_FIELDS = ("seq", "ts_kst", "epoch", "project", "session", "sid", "cwd", "agent", "gap_s",
              "mode", "m", "model", "kind", "level", "note", "new", "read")


def event(**over):
    body = {
        "mode": "codex", "model": "gpt-5.6-sol", "key": "worker", "agent": "job-7", "project": "app",
        "cwd": "app", "session": "thread-0001", "input_tokens": 120, "cache_write_input_tokens": 0,
        "cache_read_input_tokens": 4000, "output_tokens": 30, "usage_known": True,
    }
    body.update(over)
    return body


class CollectionContractTest(unittest.TestCase):
    def test_the_body_carries_exactly_the_event_fields(self):
        self.assertEqual(tuple(event()), EVENT_FIELDS)

    def test_an_event_is_taken_and_shows_in_the_feed(self):
        siren = SirenServer(self)

        status, body = siren.post("/events", event())

        self.assertEqual(status, 200)
        self.assertEqual(body["ok"], True)
        rows = siren.get("/events?limit=5")[1]["events"]
        self.assertEqual(rows[-1]["seq"], body["seq"])
        self.assertEqual((rows[-1]["mode"], rows[-1]["agent"], rows[-1]["project"], rows[-1]["sid"]),
                         ("codex", "job-7", "app", "thread-0001"))
        self.assertEqual((rows[-1]["read"], rows[-1]["new"], rows[-1]["out"]), (4000, 120, 30))

    def test_every_field_is_read(self):
        """Each field but usage_known is checked by name; usage_known false marks the usage unknown."""
        siren = SirenServer(self)
        wrong = {name: 7 for name in EVENT_FIELDS[:7]}
        wrong.update({name: -1 for name in EVENT_FIELDS[7:11]})

        for name, value in wrong.items():
            with self.subTest(field=name):
                status, body = siren.post("/events", event(**{name: value}))
                self.assertEqual(status, 400)
                self.assertIn(name, body["error"])

        status, body = siren.post("/events", event(usage_known=False))
        self.assertEqual(status, 200)
        self.assertEqual(siren.get("/events?limit=1")[1]["events"][-1]["note"], "usage_unavailable")

    def test_a_cache_write_counts_as_new_tokens(self):
        siren = SirenServer(self)

        siren.post("/events", event(input_tokens=10, cache_write_input_tokens=20, cache_read_input_tokens=0))

        self.assertEqual(siren.get("/events?limit=1")[1]["events"][-1]["new"], 30)


class ReadContractTest(unittest.TestCase):
    def test_stats_carries_what_the_console_reads(self):
        siren = SirenServer(self)
        siren.engine.quota = cache_siren.quota_from({"usage": {
            "active": {"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 1},
            "quota_slots": {"1": {"five_hour_utilization": 30, "five_hour_resets_at": 1000,
                                  "seven_day_utilization": 58, "seven_day_resets_at": 2000,
                                  "tier": "Main account", "account": "main", "provider": "claude-code",
                                  "active": True}},
        }}, "01:22")

        status, stats = siren.get("/stats")

        self.assertEqual(status, 200)
        for name in STATS_FIELDS:
            self.assertIn(name, stats)
        for name in STATS_CFG_FIELDS:
            self.assertIsInstance(stats["cfg"][name], int)
        for name in STATS_QUOTA_FIELDS:
            self.assertIn(name, stats["quota"])
        self.assertEqual(len(stats["per_min"]), 10)

    def test_every_row_carries_what_the_console_reads(self):
        siren = SirenServer(self)
        siren.post("/events", event())
        transcript_line = {
            "type": "assistant", "timestamp": "2026-01-01T00:00:00.000Z", "cwd": "/srv/app",
            "message": {"id": "msg_0001", "model": "claude-opus-5", "role": "assistant",
                        "content": [{"type": "text", "text": "Done."}],
                        "usage": {"input_tokens": 100, "output_tokens": 20,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
        }
        siren.engine.emit(siren.engine.handle(transcript_line, "11111111-2222-4333-8444-555555555555", "app", False, None))

        status, feed = siren.get("/events?limit=10")

        self.assertEqual(status, 200)
        self.assertIsInstance(feed["seq"], int)
        self.assertEqual(len(feed["events"]), 2)
        for row in feed["events"]:
            for name in ROW_FIELDS:
                with self.subTest(mode=row["mode"], field=name):
                    self.assertIn(name, row)

    def test_limit_keeps_the_newest_rows(self):
        siren = SirenServer(self)
        for n in range(3):
            siren.post("/events", event(session=f"thread-{n}"))

        rows = siren.get("/events?limit=2")[1]["events"]

        self.assertEqual([row["sid"] for row in rows], ["thread-1", "thread-2"])


if __name__ == "__main__":
    unittest.main()
