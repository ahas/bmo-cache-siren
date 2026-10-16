"""GET and POST /ttl: the cache lifetime table is read, applied live, kept in the config file, and a
bad body changes nothing. The config file is still read at start, so a writer that edits the file
and restarts the service keeps working."""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from siren_server import SirenServer, cache_siren

TABLE = {
    "claude-code": {"claude-sonnet-5": 300, "*": 3600},
    "codex": {"gpt-5.6": 1800},
    "openrouter": {},
}


class TtlEndpointTest(unittest.TestCase):
    def test_get_answers_the_table_read_at_start(self):
        siren = SirenServer(self, config={"ttl_s": 1200, "ttl_by_mode_model_s": TABLE})

        status, body = siren.get("/ttl")

        self.assertEqual(status, 200)
        self.assertEqual(body, {"ttl_s": 1200, "ttl_by_mode_model_s": TABLE})

    def test_post_applies_the_table_live(self):
        siren = SirenServer(self, config={"ttl_by_mode_model_s": TABLE})
        self.assertEqual(cache_siren.ttl_for("codex", "gpt-5.6-sol"), 1800)

        status, body = siren.post("/ttl", {"ttl_s": 900, "ttl_by_mode_model_s": {"codex": {"gpt-5.6": 600}}})

        self.assertEqual(status, 200)
        self.assertEqual((body["ok"], body["persisted"]), (True, True))
        self.assertEqual(cache_siren.ttl_for("codex", "gpt-5.6-sol"), 600)
        self.assertEqual(cache_siren.ttl_for("unlisted", "some-model"), 900)
        self.assertEqual(cache_siren.attribute("cold", set(), "gpt-5.6-sol", "gpt-5.6-sol", 601, 0, 0, mode="codex"),
                         "ttl_expired")
        self.assertEqual(siren.get("/ttl")[1], {"ttl_s": 900, "ttl_by_mode_model_s": {"codex": {"gpt-5.6": 600}}})

    def test_post_merges_only_the_ttl_keys_into_the_config_file(self):
        written = {"port": 9022, "voice_cmd": ["/opt/tts", "{text}"], "ttl_s": 3600, "ttl_by_mode_model_s": TABLE}
        siren = SirenServer(self, config=written)

        siren.post("/ttl", {"ttl_by_mode_model_s": {"claude-code": {"*": 300}}})

        on_disk = siren.config_file()
        self.assertEqual(on_disk, {**written, "ttl_by_mode_model_s": {"claude-code": {"*": 300}}})
        self.assertFalse([n for n in os.listdir(os.path.dirname(siren.config_path)) if ".tmp" in n])

    def test_a_restart_reads_back_what_was_posted(self):
        siren = SirenServer(self, config={"ttl_s": 3600})
        siren.post("/ttl", {"ttl_s": 1500})

        cache_siren.CFG.update(json.loads(json.dumps(cache_siren.DEFAULT_CONFIG)))
        cache_siren.load_config(siren.config_path)

        self.assertEqual(cache_siren.CFG["ttl_s"], 1500)

    def test_bad_bodies_are_refused_and_change_nothing(self):
        siren = SirenServer(self, config={"ttl_s": 3600, "ttl_by_mode_model_s": TABLE})
        before_file = siren.config_file()
        cases = [
            ([1, 2], "body_not_object"),
            ({}, "nothing_to_set"),
            ({"ttl_s": 60, "port": 1}, "unknown_field"),
            ({"ttl_s": 0}, "ttl_s_not_positive_integer"),
            ({"ttl_s": -5}, "ttl_s_not_positive_integer"),
            ({"ttl_s": 1.5}, "ttl_s_not_positive_integer"),
            ({"ttl_s": True}, "ttl_s_not_positive_integer"),
            ({"ttl_s": "3600"}, "ttl_s_not_positive_integer"),
            ({"ttl_by_mode_model_s": [1]}, "ttl_table_not_object"),
            ({"ttl_by_mode_model_s": {"codex": 1800}}, "ttl_table_mode_invalid"),
            ({"ttl_by_mode_model_s": {"": {}}}, "ttl_table_mode_invalid"),
            ({"ttl_by_mode_model_s": {"codex": {"": 60}}}, "ttl_table_model_invalid"),
            ({"ttl_by_mode_model_s": {"codex": {"gpt": 0}}}, "ttl_table_seconds_not_positive_integer"),
            ({"ttl_by_mode_model_s": {"codex": {"gpt": "60"}}}, "ttl_table_seconds_not_positive_integer"),
            ({"ttl_s": 60, "ttl_by_mode_model_s": {"codex": {"gpt": False}}}, "ttl_table_seconds_not_positive_integer"),
        ]

        for body, code in cases:
            with self.subTest(body=body):
                self.assertEqual(siren.post("/ttl", body), (400, {"ok": False, "error": code}))

        self.assertEqual(siren.post("/ttl", raw=b"{not json"), (400, {"ok": False, "error": "body_not_json"}))
        self.assertEqual(siren.get("/ttl")[1], {"ttl_s": 3600, "ttl_by_mode_model_s": TABLE})
        self.assertEqual(siren.config_file(), before_file)

    def test_an_empty_mode_table_is_accepted(self):
        siren = SirenServer(self)

        status, body = siren.post("/ttl", {"ttl_by_mode_model_s": {"openrouter": {}}})

        self.assertEqual(status, 200)
        self.assertIsNone(cache_siren.ttl_for("openrouter", "vendor/model"))


class ConfigReadAtStartTest(unittest.TestCase):
    def test_a_file_edited_before_start_is_what_runs(self):
        """The file-plus-restart writer: it edits the TTL keys in the file, then the service starts and reads them."""
        siren = SirenServer(self)
        written = siren.config_file()
        written.update({"ttl_s": 2400, "ttl_by_mode_model_s": {"claude-code": {"*": 2400}}})
        with open(siren.config_path, "w") as f:
            json.dump(written, f)

        cache_siren.load_config(siren.config_path)

        self.assertEqual(cache_siren.ttl_for("claude-code", "claude-opus-5"), 2400)
        self.assertEqual(cache_siren.CONFIG_PATH, siren.config_path)

    def test_a_missing_file_is_created_with_the_defaults(self):
        siren = SirenServer(self)

        self.assertEqual(siren.config_file(), json.loads(json.dumps(cache_siren.DEFAULT_CONFIG)))


if __name__ == "__main__":
    unittest.main()
