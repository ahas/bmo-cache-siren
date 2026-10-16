#!/usr/bin/env python3
"""A burst line carries the real sums of its 60-second window and lists its owners largest first."""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from cache_siren import burst_summary


class BurstSummaryTest(unittest.TestCase):
    def test_sums_new_and_read_and_orders_owners_by_new(self):
        recent = [
            (100.0, 4000, "-home-user-repos-app", 575000),
            (101.0, 250000, "-home-user--local-share-agents-search", 8000000),
            (102.0, 252000, "-home-user--local-share-agents-search", 8400000),
            (103.0, 10, "", 0),
        ]
        burst = burst_summary(recent)
        self.assertEqual(burst["new"], 506010)
        self.assertEqual(burst["read"], 16975000)
        self.assertEqual(burst["req"], 4)
        self.assertEqual([p["project"] for p in burst["parts"]],
                         ["-home-user--local-share-agents-search", "-home-user-repos-app", "?"])
        self.assertEqual(burst["parts"][0], {"project": "-home-user--local-share-agents-search",
                                             "new": 502000, "read": 16400000, "req": 2})

    def test_empty_window(self):
        self.assertEqual(burst_summary([]), {"new": 0, "read": 0, "req": 0, "parts": []})


if __name__ == "__main__":
    unittest.main()
