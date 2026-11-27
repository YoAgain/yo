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
HISTORY_FN = SOURCE[SOURCE.index("// <mesh-history-view>"):SOURCE.index("// </mesh-history-view>")]
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

    def test_rtt_shares_and_mesh_slots(self):
        script = FN + HISTORY_FN + """
          const assert = require('node:assert/strict');
          const r = userRttView([[2, 1, 3, 0, 4], [0, 0, 0, 0, 0], [0, 0, 1, 0, 0]],
                                ["first", "second", "third"], "Europe", String);
          assert.deepEqual(r.values, [30, null, 0]);
          assert.equal(r.tip(0).head, "first");
          assert.deepEqual(r.tip(0).rows.slice(1).map(r => r.value), [10, "30", 2, 1, 3, 0, 4]);
          assert.equal(meshHistoryView(null).length, 3);
          const [empty, l, east] = meshHistoryView({end_min: 30, minutes: 21, step: 10,
            links: {"yo-falkenstein>yo-tokyo": {first_min: 22, slots: [[250, 255, 2, 1], [null, 240, 0, 0]]}}});
          assert.equal(empty.rtt, undefined);
          assert.equal(east.label, "OHIO–TYO");
          assert.equal(l.label, "FSN–TYO");
          assert.equal(l.offset, 1);
          assert.deepEqual(l.rtt, [255, 240]);
          assert.deepEqual(l.fail, [2, 0]);
          assert.deepEqual(l.gaps, [1, 0]);
          assert.notEqual(l.tip(0).head, l.tip(1).head);
          assert.deepEqual(l.tip(1).rows.slice(1).map(r => r.value), ["–", "240 ms", 0, 0]);
        """
        subprocess.run(["node", "-e", script], check=True)

    def test_stale_status_is_all_red_and_none_is_none(self):
        v = view(STATUS, NOW + 60000)
        self.assertEqual({n["state"] for n in v["nodes"]} | {e["state"] for e in v["edges"]}, {"bad"})
        self.assertEqual([d["state"] for d in v["dirs"]], ["bad"] * 6)
        self.assertIsNone(view(None))


class MeshStatsTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.status = Path(tmp.name, "status.json")
        self.history = Path(tmp.name, "history.json")
        env = patch.dict(os.environ, {
            "ADMIN_USER": "test", "ADMIN_PASSWORD_HASH": "unused", "ADMIN_SESSION_KEY": "00", "ACCESS_TEAM": "invalid",
            "ACCESS_AUD": "test", "STATE_DIRECTORY": tmp.name, "MESH_STATUS_FILE": str(self.status),
            "MESH_HISTORY_FILE": str(self.history)})
        env.start()
        self.addCleanup(env.stop)
        spec = importlib.util.spec_from_file_location("admin", Path(__file__).with_name("admin.py"))
        self.admin = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.admin)

    def test_reads_the_status_file_or_none(self):
        self.assertIsNone(self.admin.mesh())
        self.status.write_text(json.dumps(STATUS))
        self.assertEqual(self.admin.mesh(), STATUS)

    def test_history_starts_at_first_ping_in_either_direction(self):
        empty = {"rtt": [None] * 23, "ok": [0] * 23, "fail": [0] * 23}
        forward = {**empty, "ok": [0] * 22 + [1], "rtt": [None] * 22 + [40]}
        reverse = {**empty, "fail": [0] * 18 + [2] + [0] * 4}
        result = self.admin.mesh_slots({"end_min": 122, "minutes": 23,
            "links": {"b>a": reverse, "a>b": forward, "b>c": empty}})
        self.assertEqual(result["links"], {"a>b": {"first_min": 118,
            "slots": [[None, None, 2, 1], [40, None, 0, 2]]}})

    def test_history_weighted_rtt_failures_gaps_and_replacement(self):
        self.assertIsNone(self.admin.mesh_history())
        link = {"rtt": [20, 60] + [None] * 9, "ok": [3, 1] + [0] * 9, "fail": [1, 2, 5] + [0] * 8}
        data = {"end_min": 20, "minutes": 11, "links": {"a>b": link}}
        self.history.write_text(json.dumps(data))
        result = self.admin.mesh_history()
        self.assertEqual(result, {**data, "step": 10, "links": {"a>b": {"first_min": 10, "slots": [[30, None, 8, 7], [None, None, 0, 1]]}}})
        self.assertIs(self.admin.mesh_history(), result)
        self.status.write_text(json.dumps({**data, "end_min": 21}))
        self.status.replace(self.history)
        self.assertEqual(self.admin.mesh_history()["end_min"], 21)



if __name__ == "__main__":
    unittest.main()
