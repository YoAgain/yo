"""Build colours by support: the newest allowed build green through the oldest allowed yellow, below the minimum red."""
import json
import re
import subprocess
import unittest
from pathlib import Path


class ReleaseColorTests(unittest.TestCase):
    def test_scale(self):
        src = Path(__file__).with_name("app.js").read_text()
        fn = re.search(r"// @release-color\n(.*?)// @end", src, re.S).group(1)
        calls = "[releaseColor(0, 3), releaseColor(1, 3), releaseColor(2, 3), releaseColor(0, 1), releaseColor(-1, 3, true)]"
        out = subprocess.run(["node", "-e", fn + f"console.log(JSON.stringify({calls}))"],
                             capture_output=True, text=True, check=True).stdout
        c = json.loads(out)
        hue = [int(re.search(r"hsl\((\d+)", x).group(1)) for x in c]
        self.assertEqual(hue[0], 135)  # newest: green
        self.assertTrue(hue[0] > hue[1] > hue[2] >= 45)  # oldest still taken: yellow
        self.assertEqual(hue[3], 135)  # the only one: green
        self.assertEqual(hue[4], 4)  # below the minimum: red


if __name__ == "__main__":
    unittest.main()
