"""The Mesh card (app.js's pure part, run in node) and the mesh part of stats.json: the three servers from yo-mesh's
status file, each line the round trip of both directions, green or red; a stale file makes everything red."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SOURCE = (Path(__file__).with_name("app.js")).read_text()
FN = SOURCE[SOURCE.index("// <mesh-view>"):SOURCE.index("// </mesh-view>")]
NOW = 1_800_000_000_000


def up(rtt):
    return {"online": True, "rtt_ms": rtt, "last_ok_ms": NOW - 1000, "error": None}


DOWN = {"online": False, "rtt_ms": None, "last_ok_ms": NOW - 60000, "error": "ping timed out"}
STATUS = {"node": "yo-falkenstein", "version": "0.1.0", "at_ms": NOW - 1000, "peers": {
    "yo-tokyo": {**up(252.0), "view": {"yo-falkenstein": up(254.0), "yo-ohio": DOWN}},
    "yo-ohio": {**up(107.0), "view": {"yo-falkenstein": up(106.0), "yo-tokyo": DOWN}}}}


def view(status, now=NOW):
    script = FN + "\nconst c = JSON.parse(process.argv[1]);\nconsole.log(JSON.stringify(meshView(c.m, c.now)));\n"
    out = subprocess.run(["node", "-e", script, json.dumps({"m": status, "now": now})], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


@unittest.skipUnless(shutil.which("node"), "node not installed")
class MeshViewTests(unittest.TestCase):
    def test_lines_both_directions_green_or_red_details_in_the_hover(self):
        v = view(STATUS)
        self.assertEqual([(n["label"], n["state"]) for n in v["nodes"]], [("FSN", "ok"), ("OHIO", "ok"), ("TYO", "ok")])
        self.assertEqual([(e["a"], e["b"], e["rtt"], e["state"]) for e in v["edges"]],
                         [("yo-falkenstein", "yo-ohio", 107, "ok"), ("yo-falkenstein", "yo-tokyo", 253, "ok"),
                          ("yo-ohio", "yo-tokyo", None, "bad")])
        self.assertEqual((v["up"], v["total"]), (3, 3))
        self.assertEqual([d["label"] for d in v["dirs"]],
                         ["FSN → OHIO", "OHIO → FSN", "FSN → TYO", "TYO → FSN", "OHIO → TYO", "TYO → OHIO"])
        self.assertEqual([d["value"] for d in v["dirs"]], ["107 ms", "106 ms", "252 ms", "254 ms", "down", "down"])
        self.assertIn({"label": "Error", "value": "ping timed out"}, v["dirs"][4]["tip"]["rows"])
        rows = [(r.get("label"), r.get("value")) for r in v["rows"]]
        self.assertIn(("FSN → OHIO", "107 ms"), rows)
        self.assertIn(("OHIO → TYO", "down"), rows)
        self.assertIn(("Error", "ping timed out"), rows)
        self.assertFalse(v["ok"])

    def test_stale_status_is_all_red_and_none_is_none(self):
        v = view(STATUS, NOW + 60000)
        self.assertEqual({n["state"] for n in v["nodes"]} | {e["state"] for e in v["edges"]}, {"bad"})
        self.assertEqual([d["state"] for d in v["dirs"]], ["bad"] * 6)
        self.assertIsNone(view(None))


class MeshStatsTests(unittest.TestCase):
    def test_reads_the_status_file_or_none(self):
        with tempfile.TemporaryDirectory() as d, patch.dict(os.environ, {
                "ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00", "ACCESS_TEAM": "invalid",
                "ACCESS_AUD": "test", "STATE_DIRECTORY": d, "MESH_STATUS_FILE": os.path.join(d, "status.json")}):
            spec = importlib.util.spec_from_file_location("admin", Path(__file__).with_name("admin.py"))
            admin = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(admin)
            self.assertIsNone(admin.mesh())
            Path(d, "status.json").write_text(json.dumps(STATUS))
            self.assertEqual(admin.mesh(), STATUS)


if __name__ == "__main__":
    unittest.main()
