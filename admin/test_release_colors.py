"""Build colours by age: green shades under 24 h, green-yellow to orange up to a week, red from a week."""
import json
import re
import subprocess
import unittest
from pathlib import Path


class ReleaseColorTests(unittest.TestCase):
    def test_scale(self):
        src = Path(__file__).with_name("app.js").read_text()
        fn = re.search(r"// @release-color\n(.*?)// @end", src, re.S).group(1)
        hours = [0, 12, 23, 24, 96, 167, 168, 1000]
        out = subprocess.run(["node", "-e", fn + f"console.log(JSON.stringify({hours}.map(releaseColor)))"],
                             capture_output=True, text=True, check=True).stdout
        c = json.loads(out)
        hue = [int(re.search(r"hsl\((\d+)", x).group(1)) for x in c]
        self.assertTrue(all(h == 135 for h in hue[:3]) and len(set(c[:3])) == 3)
        self.assertTrue(hue[3] > hue[4] > hue[5] >= 28)
        self.assertEqual(hue[6], 4); self.assertEqual(c[6], c[7])


if __name__ == "__main__":
    unittest.main()
