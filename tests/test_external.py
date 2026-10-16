import json
import os
import sys
import tempfile
import threading
import unittest
from collections import deque

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from cache_siren import Engine, load_recent_events


class DurableRingTests(unittest.TestCase):
    def test_last_valid_rows_survive_broken_lines(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "events.jsonl")
            with open(path, "w") as out:
                out.write(json.dumps({"seq": 1, "model": "old"}) + "\n")
                out.write("not-json\n")
                out.write(json.dumps({"seq": 2, "model": "middle"}) + "\n")
                out.write(json.dumps({"seq": 3, "model": "new"}) + "\n")

            self.assertEqual([row["model"] for row in load_recent_events(path, 2)], ["middle", "new"])


class ExternalEventTests(unittest.TestCase):
    def engine(self):
        engine = Engine.__new__(Engine)
        engine.ring = deque(maxlen=20)
        engine.cond = threading.Condition()
        return engine

    def test_codex_keeps_the_reported_split_and_agent(self):
        event = self.engine().external({
            "mode": "codex",
            "model": "gpt-5.6-sol",
            "key": "bmo",
            "agent": "chat:7",
            "project": "bmo",
            "cwd": "bmo",
            "session": "thread-1",
            "input_tokens": 10,
            "cache_write_input_tokens": 20,
            "cache_read_input_tokens": 30,
            "output_tokens": 4,
            "usage_known": True,
        })

        self.assertEqual(event["mode"], "codex")
        self.assertEqual(event["agent"], "chat:7")
        self.assertEqual(event["new"], 30)
        self.assertEqual(event["read"], 30)
        self.assertEqual(event["out"], 4)
        self.assertEqual(event["backend"], "openai")

    def test_openrouter_unknown_usage_is_not_invented(self):
        event = self.engine().external({
            "mode": "openrouter",
            "model": "vendor/model",
            "key": "classifier",
            "usage_known": False,
        })

        self.assertEqual(event["kind"], "request")
        self.assertEqual(event["note"], "usage_unavailable")
        self.assertEqual(event["new"], 0)
        self.assertEqual(event["read"], 0)

    def test_rejects_a_provider_that_has_no_ingestion_contract(self):
        with self.assertRaisesRegex(ValueError, "mode"):
            self.engine().external({"mode": "claude-code", "model": "x", "key": "bmo"})


if __name__ == "__main__":
    unittest.main()
