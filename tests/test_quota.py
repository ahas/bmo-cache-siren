#!/usr/bin/env python3
"""The header's quota % is that of the account the source marks as supplying now."""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from cache_siren import quota_from


def slot(tier, account, five, week, active=False):
    return {"five_hour_utilization": five, "five_hour_resets_at": 1000,
            "seven_day_utilization": week, "seven_day_resets_at": 2000,
            "tier": tier, "account": account, "provider": "claude-code", "active": active}


class QuotaFromTest(unittest.TestCase):
    def test_reads_the_slot_marked_active_even_when_it_is_not_first(self):
        payload = {"usage": {
            "active": {"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 2},
            "quota_slots": {
                "1": slot("Default account", "default", 96, 99),
                "2": slot("Main account", "main", 30, 58, active=True),
            },
        }}

        quota = quota_from(payload, "01:22")

        self.assertEqual((quota["five_h"], quota["seven_d"], quota["tier"]), (30, 58, "Main account"))

    def test_reads_the_slot_the_active_account_names_when_no_slot_is_marked(self):
        payload = {"usage": {
            "active": {"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 2},
            "quota_slots": {
                "1": {**slot("Default account", "default", 96, 99), "active": None},
                "2": {**slot("Main account", "main", 30, 58), "active": None},
            },
        }}

        quota = quota_from(payload, "01:22")

        self.assertEqual((quota["five_h"], quota["account"], quota["accounts"]), (30, "main", 2))

    def test_a_source_with_one_bare_slot_reads_that_slot(self):
        payload = {"usage": {"quota_slots": {"1": {"five_hour_utilization": 12, "seven_day_utilization": 40}}}}

        quota = quota_from(payload, "01:22")

        self.assertEqual((quota["five_h"], quota["seven_d"], quota["account"], quota["accounts"]), (12, 40, None, None))

    def test_reads_the_first_slot_when_the_active_one_leads(self):
        payload = {"usage": {
            "active": {"provider": "claude-code", "account": "main", "name": "Main account", "accounts": 2},
            "quota_slots": {
                "1": slot("Main account", "main", 30, 58, active=True),
                "2": slot("Default account", "default", 96, 99),
            },
        }}

        quota = quota_from(payload, "01:22")

        self.assertEqual((quota["five_h"], quota["seven_d"], quota["tier"]), (30, 58, "Main account"))


if __name__ == "__main__":
    unittest.main()
