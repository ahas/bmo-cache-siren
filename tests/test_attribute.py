#!/usr/bin/env python3
"""Tests of the attribute() cause ladder, standard unittest only (no daemon is started).

    python3 -m unittest discover -s tests -v      # or  python3 tests/test_attribute.py

The values come from real requests left in events.jsonl.
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from cache_siren import CFG, attribute, ttl_for

TTL = CFG["ttl_s"]


class Attribute(unittest.TestCase):
    def test_ttl_is_chosen_by_mode_and_longest_model_prefix(self):
        config = {
            "ttl_s": 99,
            "ttl_by_mode_model_s": {
                "claude-code": {"claude-sonnet": 600, "claude-sonnet-5": 300, "*": 3600},
                "codex": {"gpt-5.6": 1800},
                "openrouter": {},
            },
        }

        self.assertEqual(ttl_for("claude-code", "claude-sonnet-5-20260901", config), 300)
        self.assertEqual(ttl_for("claude-code", "claude-opus-5", config), 3600)
        self.assertEqual(ttl_for("codex", "gpt-5.6-sol", config), 1800)
        self.assertIsNone(ttl_for("openrouter", "anthropic/claude-sonnet-5", config))
        self.assertEqual(ttl_for("unlisted", "some-model", config), 99)

    def test_sonnet_follows_the_same_default_hour_as_opus(self):
        self.assertEqual(
            attribute("cold", set(), "claude-sonnet-5", "claude-sonnet-5", 301, 0, 0),
            "unknown")
        self.assertEqual(
            attribute("cold", set(), "claude-sonnet-5", "claude-sonnet-5", TTL + 1, 0, 0),
            "ttl_expired")

    def test_owner_report_is_prefix_changed(self):
        """09-05 17:53:29 (c36b6e6f): a gap of 3382s is inside an hour, and it held 34177 but read only 2315.
        The cache was alive, so this is not ttl_expired but a changed prefix."""
        self.assertEqual(
            attribute("growth", set(), "claude-opus-5", "claude-opus-5", 3382, 2315, 34177),
            "prefix_changed")

    def test_gap_past_ttl_is_ttl_expired(self):
        self.assertEqual(
            attribute("cold", set(), "claude-opus-5", "claude-opus-5", TTL + 1, 0, 40410),
            "ttl_expired")

    def test_gap_exactly_at_ttl_is_not_ttl_expired(self):
        """Expiry needs a gap past the TTL: a gap exactly at it still has the cache."""
        self.assertNotEqual(
            attribute("growth", set(), "claude-opus-5", "claude-opus-5", TTL, 62948, 62950),
            "ttl_expired")

    def test_reused_prefix_with_big_read_keeps_its_name(self):
        """A request that read all it held and added a lot on top (09-05 07:19:23 ad96ad24) = just a big Read."""
        self.assertEqual(
            attribute("growth", set(), "claude-opus-5", "claude-opus-5", TTL - 1, 62948, 62950),
            "")
        self.assertEqual(
            attribute("cold", {"tool_search"}, "claude-opus-5", "claude-opus-5", 600, 308616, 308618),
            "tool_search")

    def test_first_request_is_session_start(self):
        self.assertEqual(
            attribute("start", set(), "claude-opus-5", None, None, 0, 0),
            "session_start")

    def test_model_switch_wins(self):
        self.assertEqual(
            attribute("cold", set(), "claude-sonnet-5", "claude-opus-5", 120, 0, 34177),
            "model_switch")
        self.assertEqual(
            attribute("growth", {"refusal"}, "claude-opus-5", "claude-opus-5", 120, 2315, 34177),
            "model_switch")

    def test_compact_wins(self):
        self.assertEqual(
            attribute("cold", {"compact"}, "claude-opus-5", "claude-opus-5", 120, 0, 34177),
            "compact")
        self.assertEqual(
            attribute("cold", {"effort_model"}, "claude-opus-5", "claude-opus-5", 120, 0, 34177),
            "effort_model")

    def test_cold_inside_ttl_is_prefix_changed(self):
        """Inside the cache lifetime yet nothing was read = the whole prefix moved."""
        self.assertEqual(
            attribute("cold", set(), "claude-opus-5", "claude-opus-5", 300, 0, 693617),
            "prefix_changed")

    def test_unknown_survives_for_a_first_context(self):
        """With no previous context (the first usage after an error or a limit) there is no ground to judge the prefix."""
        self.assertEqual(
            attribute("cold", set(), "claude-opus-5", "claude-opus-5", 300, 0, 0),
            "unknown")

    def test_ttl_is_configurable(self):
        self.assertEqual(
            attribute("cold", set(), "claude-opus-5", "claude-opus-5", 3400, 0, 0, ttl_s=3300),
            "ttl_expired")
        self.assertEqual(
            attribute("cold", set(), "claude-opus-5", "claude-opus-5", 3400, 0, 0, ttl_s=7200),
            "unknown")

    def test_other_kinds_get_no_name(self):
        self.assertEqual(
            attribute("warm", set(), "claude-opus-5", "claude-opus-5", 7200, 100, 200000),
            "")


if __name__ == "__main__":
    unittest.main()
