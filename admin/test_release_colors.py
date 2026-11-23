"""Release colours: green only for the newest release (and two lighter ones), then yellow, orange, red by rank."""
import re
import subprocess
import unittest
from pathlib import Path


class ReleaseColorTests(unittest.TestCase):
    def test_scale(self):
        src = Path(__file__).with_name("app.js").read_text()
        fn = re.search(r"// @release-color\n(.*?)// @end", src, re.S).group(1)
        out = subprocess.run(["node", "-e", fn + "console.log(JSON.stringify([...Array(12).keys()].map(releaseColor)))"],
                             capture_output=True, text=True, check=True).stdout
        import json
        c = json.loads(out)
        hue = [int(re.search(r"hsl\((\d+)", x).group(1)) for x in c]
        self.assertTrue(all(100 <= h <= 140 for h in hue[:3]) and len(set(c[:3])) == 3)
        self.assertTrue(all(h < 100 for h in hue[3:]))
        self.assertEqual(len({c[4], c[5]}), 1)
        self.assertEqual(len({c[6], c[9]}), 1)
        self.assertEqual(len({c[10], c[11]}), 1)
        self.assertEqual(hue[4], 48); self.assertEqual(hue[6], 28); self.assertEqual(hue[10], 4)


if __name__ == "__main__":
    unittest.main()
