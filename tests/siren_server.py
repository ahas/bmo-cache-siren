"""A real cache-siren HTTP server for one test: an ephemeral loopback port, a temporary data
directory and a fresh copy of the defaults, all put back when the test ends. No transcript is
scanned and no background loop is started; only the request handler runs."""
import json
import os
import sys
import tempfile
import threading
import urllib.error
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

import cache_siren

PATHS = ("RUN_DIR", "LOG", "EVENTS", "STATE", "CTL", "SIREN_WAV")


class SirenServer:
    def __init__(self, testcase, config=None):
        """Start a server for `testcase`; it is taken down by the test's own cleanup. `config`, when
        given, is written to the config file and read the way the daemon reads it at start."""
        self.dir = tempfile.TemporaryDirectory()
        testcase.addCleanup(self.dir.cleanup)
        root = self.dir.name

        saved = {name: getattr(cache_siren, name) for name in PATHS + ("ENGINE", "QUIET", "CONFIG_PATH")}
        saved_cfg = dict(cache_siren.CFG)

        def restore():
            for name, value in saved.items():
                setattr(cache_siren, name, value)
            cache_siren.CFG.clear()
            cache_siren.CFG.update(saved_cfg)

        testcase.addCleanup(restore)

        cache_siren.RUN_DIR = root
        cache_siren.LOG = os.path.join(root, "cache-siren.log")
        cache_siren.EVENTS = os.path.join(root, "events.jsonl")
        cache_siren.STATE = os.path.join(root, "state.json")
        cache_siren.CTL = os.path.join(root, "ctl.json")
        cache_siren.SIREN_WAV = os.path.join(root, "siren.wav")
        cache_siren.QUIET = True
        cache_siren.CFG.clear()
        cache_siren.CFG.update(json.loads(json.dumps(cache_siren.DEFAULT_CONFIG)))
        cache_siren.CFG.update({"sound": False, "voice": False, "quota_url": "", "heartbeat_url": ""})

        self.config_path = os.path.join(root, "config.json")
        if config is not None:
            with open(self.config_path, "w") as f:
                json.dump(config, f)
        cache_siren.load_config(self.config_path)

        self.engine = cache_siren.Engine()
        cache_siren.ENGINE = self.engine

        self.server = cache_siren.Server(("127.0.0.1", 0), cache_siren.Handler)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        thread = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()

        def stop():
            self.server.shutdown()
            self.server.server_close()
            thread.join()

        testcase.addCleanup(stop)

    def call(self, method, path, body=None, raw=None):
        """One request; returns (status, decoded JSON body) for success and error answers alike."""
        data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
        request = urllib.request.Request(self.base + path, data=data, method=method,
                                         headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as e:
            with e:
                return e.code, json.load(e)

    def get(self, path):
        return self.call("GET", path)

    def post(self, path, body=None, raw=None):
        return self.call("POST", path, body, raw)

    def config_file(self):
        with open(self.config_path) as f:
            return json.load(f)
