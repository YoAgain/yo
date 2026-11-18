import json, pathlib, sys
meta, commit, dirty, root = json.load(open(sys.argv[1])), sys.argv[2], sys.argv[3], pathlib.Path(sys.argv[4])
packages = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
start = next(p["id"] for p in meta["packages"] if p["name"] == "yo-bindings")
# What the app links: yo-bindings and its normal dependencies, not build scripts' or tests'.
seen, todo = set(), [start]
while todo:
    node = nodes[todo.pop()]
    for dep in node["deps"]:
        if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
            seen.add(dep["pkg"])
            todo.append(dep["pkg"])
crates = sorted((packages[i] for i in seen if packages[i]["source"]), key=lambda p: (p["name"], p["version"]))
out = [
    f"Source: https://github.com/YoAgain/yo/tree/{commit}{dirty}",
    "",
    "Yo",
    "==",
    (root / "LICENSE.md").read_text().strip(),
    "",
    "The name Yo, its logo and its sound are not covered by this licence.",
    "",
    "libsignal",
    "=========",
    "The app links libsignal (https://github.com/signalapp/libsignal), Copyright Signal Messenger, LLC, under the GNU",
    "Affero General Public License version 3 (below). The app as a whole is distributed under the AGPL-3.0; its",
    "complete corresponding source is the repository at the commit above.",
    "",
    "Montserrat",
    "==========",
    "The Montserrat font, Copyright The Montserrat Project Authors, under the SIL Open Font License 1.1",
    "(https://openfontlicense.org).",
    "",
    "Rust crates",
    "===========",
]
# Each licence text once (most crates carry the same MIT and Apache texts), numbered; a crate names its texts.
texts = {}
for p in crates:
    numbers = []
    folder = pathlib.Path(p["manifest_path"]).parent
    for f in sorted(folder.iterdir()):
        name = f.name.upper()
        if f.is_file() and name.startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")):
            text = f.read_text(errors="replace").strip()
            if "GNU AFFERO GENERAL PUBLIC LICENSE" in text:
                continue  # the AGPL, once, at the end
            numbers.append(texts.setdefault(text, len(texts) + 1))
    shown = ", ".join(f"[{n}]" for n in sorted(set(numbers)))
    out.append(f"{p['name']} {p['version']}: {p.get('license') or 'see its texts'} {shown} {p.get('repository') or ''}".rstrip())
for text, n in texts.items():
    out += ["", f"[{n}]", text]
out += ["", "GNU Affero General Public License, version 3", "=" * 44, (root / "LICENSES/AGPL-3.0.txt").read_text()]
print("\n".join(out))
