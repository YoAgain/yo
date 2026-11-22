# Makes Yo's custom SF Symbols (.symbolset, template v3: Ultralight/Regular/Black small masters with margins)
# from Montserrat's outlines (variable font, so the three masters are point-compatible and interpolate).
# The Regular master is Montserrat Bold (the brand's), Ultralight wght 250, Black wght 900; sizes are balanced against
# camera.fill (Yo Camera) in the circular controls.
# usage: uv run --with uharfbuzz dev/symbols/make_symbols.py <Montserrat[wght].ttf> YoApp/Resources/Symbols.xcassets
#   (the variable font: github.com/google/fonts, ofl/montserrat/Montserrat[wght].ttf)
# Check: dev/symbols/preview.swift renders the four controls from the compiled catalog (actool on a Mac).
import json, os, sys
import uharfbuzz as hb

FONT, OUT = sys.argv[1], sys.argv[2]
# Master weight -> Montserrat wght: the Regular symbol is the brand's Montserrat Bold.
MASTERS = {"Ultralight": 250, "Regular": 700, "Black": 900}
COLX = {"Ultralight": 560.0, "Regular": 1400.0, "Black": 2880.0}   # column origins of the template
BASE = 696.0          # Baseline-S
CAP = 70.459          # SF cap height at the template's 100 pt
MARGIN = {"Ultralight": 6.0, "Regular": 6.0, "Black": 6.0}
# Wordmark size: Montserrat's cap (700/1000) scaled to this many template units.
SIZES = {"yo.wordmark": 64.0, "yo.back": 56.0, "yo.letter": 74.0}
# Arrow shaft thickness per master (template units), matched to Montserrat's stems at that weight.
ARROW_T = {"Ultralight": 0.06, "Regular": 0.17, "Black": 0.22}   # x the cap height

blob = hb.Blob.from_file_path(FONT)
face = hb.Face(blob)


class Pen:
  def __init__(self, sx, sy, ox, oy):
    self.d, self.s, self.o = [], (sx, sy), (ox, oy)
  def p(self, x, y):
    return f"{x * self.s[0] + self.o[0]:.3f} {self.o[1] - y * self.s[1]:.3f}"
  def moveTo(self, a): self.d.append("M" + self.p(*a))
  def lineTo(self, a): self.d.append("L" + self.p(*a))
  def qCurveTo(self, a, b): self.d.append("Q" + self.p(*a) + " " + self.p(*b))
  def curveTo(self, a, b, c): self.d.append("C" + " ".join([self.p(*a), self.p(*b), self.p(*c)]))
  def closePath(self): self.d.append("Z")


def text_paths(text, wght, size):
  """Outlines of text (shaped, kerned) in template units, origin at the left of the pen, y up from baseline."""
  font = hb.Font(face)
  font.set_variations({"wght": wght})
  buf = hb.Buffer(); buf.add_str(text); buf.guess_segment_properties()
  hb.shape(font, buf, {"kern": True, "liga": False})
  s = size / 700.0       # 700 = Montserrat cap height
  out, x = [], 0.0
  for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
    pen = Pen(s, s, (x + pos.x_offset) * s, 0)
    font.draw_glyph_with_pen(info.codepoint, pen)
    out.append(" ".join(pen.d))
    x += pos.x_advance
  return out


def bbox(paths):
  import re
  nums = [float(n) for p in paths for n in re.findall(r"-?\d+\.?\d*", p)]
  xs, ys = nums[0::2], nums[1::2]
  return min(xs), min(ys), max(xs), max(ys)   # y is svg-down here (relative to baseline 0)


def arrow(t, head_len, head_half, x0, y0, run, radius, drop):
  """Filled 'turn up left' arrow: tip at (x0, y0) pointing left, shaft right, turning down. svg coords (y down)."""
  k = 0.5523
  h = t / 2
  xa = x0 + head_len            # head base
  xc = xa + run                 # where the turn starts
  ro, ri = radius + h, radius - h
  cy = y0 + radius              # turn centre y
  yb = y0 + drop                # end of the tail
  P = lambda x, y: f"{x:.3f} {y:.3f}"
  return (
    f"M{P(x0, y0)} L{P(xa, y0 - head_half)} L{P(xa, y0 - h)} L{P(xc, y0 - h)} "
    f"C{P(xc + k * ro, y0 - h)} {P(xc + ro, cy - k * ro)} {P(xc + ro, cy)} "
    f"L{P(xc + ro, yb)} L{P(xc + ri, yb)} L{P(xc + ri, cy)} "
    f"C{P(xc + ri, cy - k * ri)} {P(xc + k * ri, y0 + h)} {P(xc, y0 + h)} "
    f"L{P(xa, y0 + h)} L{P(xa, y0 + head_half)} Z")


def shapes(name, weight):
  w = MASTERS[weight]
  size = SIZES[name]
  if name == "yo.wordmark":
    return text_paths("Yo", w, size)
  if name == "yo.letter":
    return text_paths("Y", w, size)
  # yo.back: the wordmark under a reply arrow (arrowshape.turn.up.left's shape) that turns down into the "o".
  yo = text_paths("Yo", w, size)
  x0, _, x1, _ = bbox(yo)
  t = ARROW_T[weight] * size
  xh = size * 517 / 700           # x-height
  o_left = x0 + (x1 - x0) * 0.52  # roughly where the o starts
  o_mid = (o_left + x1) / 2
  head_half = t * 0.5 + size * 0.15
  head_len = head_half * 1.1
  gap = size * 0.10
  arr_y = -(size + gap + head_half)          # the head clears the cap line
  tip_x = x0 + (x1 - x0) * 0.0
  radius = size * 0.2
  right = o_mid + t / 2
  run = right - radius - (tip_x + head_len)
  drop = (-(xh + gap)) - arr_y               # the tail ends a gap above the o
  return yo + [arrow(t, head_len, head_half, tip_x, arr_y, run, radius, drop)]


def template(name):
  guides, groups = [], []
  for weight in ("Ultralight", "Regular", "Black"):
    ps = shapes(name, weight)
    x0, y0, x1, y1 = bbox(ps)
    ox = COLX[weight] - (x0 + x1) / 2      # centre each master on its column
    m = MARGIN[weight]
    guides.append(f'<line id="left-margin-{weight}-S" style="fill:none;stroke:#00AEEF;stroke-width:0.5;opacity:1.0;" '
                  f'x1="{x0 + ox - m:.3f}" x2="{x0 + ox - m:.3f}" y1="600.785" y2="720.121"/>')
    guides.append(f'<line id="right-margin-{weight}-S" style="fill:none;stroke:#00AEEF;stroke-width:0.5;opacity:1.0;" '
                  f'x1="{x1 + ox + m:.3f}" x2="{x1 + ox + m:.3f}" y1="600.785" y2="720.121"/>')
    body = "\n".join(f'   <path d="{p}"/>' for p in ps)
    groups.append(f'  <g id="{weight}-S" transform="matrix(1 0 0 1 {ox:.3f} {BASE})">\n{body}\n  </g>')
  lines = "\n".join(
    f'  <line id="{n}" style="fill:none;stroke:#27AAE1;opacity:1;stroke-width:0.5;" x1="263" x2="3036" y1="{y}" y2="{y}"/>'
    for n, y in [("Baseline-S", 696), ("Capline-S", 625.541), ("Baseline-M", 1126), ("Capline-M", 1055.54),
                 ("Baseline-L", 1556), ("Capline-L", 1485.54)])
  return f'''<?xml version="1.0" encoding="UTF-8"?>
<!--Generated by dev/symbols/make_symbols.py from Montserrat (OFL)-->
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg version="1.1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="3300" height="2200">
 <!--glyph: "{name}", point size: 100.0, template writer version: "1"-->
 <g id="Notes">
  <text style="stroke:none;fill:black;font-family:sans-serif;font-size:13;" transform="matrix(1 0 0 1 263 1933)">Template v.3.0</text>
  <text id="template-version" style="stroke:none;fill:black;font-family:sans-serif;font-size:13;" transform="matrix(1 0 0 1 3036 1933)">Template v.3.0</text>
 </g>
 <g id="Guides">
{lines}
  {chr(10).join("  " + g for g in guides).strip()}
 </g>
 <g id="Symbols">
{chr(10).join(groups)}
 </g>
</svg>
'''


os.makedirs(OUT, exist_ok=True)
with open(os.path.join(OUT, "Contents.json"), "w") as f:
  json.dump({"info": {"author": "xcode", "version": 1}}, f, indent=2)
for name in SIZES:
  d = os.path.join(OUT, name + ".symbolset")
  os.makedirs(d, exist_ok=True)
  with open(os.path.join(d, name + ".svg"), "w") as f:
    f.write(template(name))
  with open(os.path.join(d, "Contents.json"), "w") as f:
    json.dump({"info": {"author": "xcode", "version": 1},
               "symbols": [{"filename": name + ".svg", "idiom": "universal"}]}, f, indent=2)
print("ok", OUT)
