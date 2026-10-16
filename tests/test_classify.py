#!/usr/bin/env python3
"""Tests of how `<synthetic>` lines are classified, standard unittest only (no daemon is started).

    python3 -m unittest discover -s tests -v      # or  python3 tests/test_classify.py

The values come from lines really left in ~/.claude/projects transcripts.
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from cache_siren import Engine


def synthetic(text, is_api_error, ts="2026-01-01T00:00:00.000Z"):
    """One `<synthetic>` assistant line as the CLI leaves it in a transcript."""
    return {
        "type": "assistant",
        "timestamp": ts,
        "isApiErrorMessage": is_api_error,
        "cwd": "/srv/app",
        "message": {
            "id": None,
            "model": "<synthetic>",
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {"input_tokens": 0, "output_tokens": 0,
                      "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
        },
    }


class SyntheticLines(unittest.TestCase):
    def engine(self):
        engine = Engine.__new__(Engine)
        engine.sessions = {}
        return engine

    def read(self, record, sid="11111111-2222-4333-8444-555555555555"):
        return self.engine().handle(record, sid, "bmo", False, None)

    def test_a_fold_leaves_a_notice_and_not_an_error(self):
        ev = self.read(synthetic("No response requested.", False))

        self.assertEqual(ev["kind"], "notice")
        self.assertEqual(ev["level"], "info")
        self.assertEqual(ev["note"], "No response requested.")

    def test_an_api_error_is_still_an_error(self):
        ev = self.read(synthetic(
            "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.",
            True))

        self.assertEqual(ev["kind"], "error")
        self.assertEqual(ev["level"], "yellow")

    def test_a_session_limit_is_still_quota(self):
        ev = self.read(synthetic("You've hit your session limit · resets 8:10pm (UTC)", True))

        self.assertEqual(ev["kind"], "quota")
        self.assertEqual(ev["level"], "yellow")

    def test_a_weekly_limit_is_quota_and_not_an_error(self):
        ev = self.read(synthetic("You've hit your weekly limit · resets 6am (UTC)", True))

        self.assertEqual(ev["kind"], "quota")
        self.assertEqual(ev["level"], "yellow")

    def test_a_monthly_spend_limit_is_quota_and_not_an_error(self):
        ev = self.read(synthetic(
            "You've hit your monthly spend limit · raise it at claude.ai/settings/usage", True))

        self.assertEqual(ev["kind"], "quota")

    def test_a_failure_that_names_no_limit_is_still_an_error(self):
        ev = self.read(synthetic("Not logged in · Please run /login", True))

        self.assertEqual(ev["kind"], "error")

    def test_a_notice_does_not_swallow_the_error_that_follows_it(self):
        engine = self.engine()
        sid = "11111111-2222-4333-8444-555555555555"
        engine.handle(synthetic("No response requested.", False), sid, "bmo", False, None)
        ev = engine.handle(synthetic("API Error: Connection lost mid-response. The response above may be incomplete.",
                                     True, ts="2026-01-01T00:00:30.000Z"), sid, "bmo", False, None)

        self.assertEqual(ev["kind"], "error")


if __name__ == "__main__":
    unittest.main()
