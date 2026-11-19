"""Finds the newest version of every third-party dependency and, unless --dry-run, pins it in place.

Used by ./update-deps.sh. crates.io dependencies (every Cargo.toml: the workspace, its members, fuzz) are pinned
exactly (`=x.y.z`); git dependencies with a tag (libsignal, if the core takes it again) get the newest release tag and
its commit as `rev`, and so do their [patch] entries. Prints one line per change with where its release notes are.
Standard library only.
"""

import json
import re
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFESTS = [p for p in ROOT.glob("**/Cargo.toml") if "target" not in p.parts and ".claude" not in p.relative_to(ROOT).parts]
SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
AGENT = "yo-update-deps (manual dependency update script)"
_crates = {}


def crate(name):
    if name not in _crates:
        time.sleep(1)  # crates.io asks for at most one request a second
        req = urllib.request.Request(f"https://crates.io/api/v1/crates/{name}", headers={"User-Agent": AGENT})
        with urllib.request.urlopen(req, timeout=30) as r:
            data = json.load(r)["crate"]
        _crates[name] = (data["max_stable_version"] or data["max_version"], data.get("repository") or "")
    return _crates[name]


def newest_tag(url):
    """The newest release tag of a git repository (vX.Y.Z, no pre-releases) and its commit."""
    out = subprocess.run(["git", "ls-remote", "--tags", url], capture_output=True, text=True, check=True).stdout
    tags = {}
    for line in out.splitlines():
        sha, ref = line.split("\t")
        m = re.fullmatch(r"refs/tags/(v?(\d+)\.(\d+)\.(\d+))(\^\{\})?", ref)
        if m:
            # An annotated tag's commit is the peeled ^{} line: it wins over the tag object.
            if m.group(5) or m.group(1) not in tags:
                tags[m.group(1)] = (tuple(map(int, m.group(2, 3, 4))), sha)
    tag = max(tags, key=lambda t: tags[t][0])
    return tag, tags[tag][1]


def tables(doc):
    """(section path, deps table) for every dependency table of a manifest, [patch.*] included."""
    for s in SECTIONS:
        if s in doc:
            yield s, doc[s]
    if "dependencies" in doc.get("workspace", {}):
        yield "workspace.dependencies", doc["workspace"]["dependencies"]
    for target, t in doc.get("target", {}).items():
        for s in SECTIONS:
            if s in t:
                yield f"target.{target}.{s}", t[s]
    for source, t in doc.get("patch", {}).items():
        yield f"patch.{source}", t


def plan():
    changes = []  # (manifest, dep, kind, old, new, notes)
    for path in MANIFESTS:
        doc = tomllib.loads(path.read_text())
        for _, deps in tables(doc):
            for dep, spec in deps.items():
                if isinstance(spec, str):
                    spec = {"version": spec}
                if spec.get("workspace") or "path" in spec:
                    continue
                name = spec.get("package", dep)
                if "git" in spec:
                    if "tag" not in spec:
                        continue
                    tag, rev = newest_tag(spec["git"])
                    if tag != spec["tag"] or rev != spec.get("rev"):
                        notes = spec["git"].removesuffix(".git") + f"/releases/tag/{tag}"
                        changes.append((path, dep, "git", (spec["tag"], spec.get("rev")), (tag, rev), notes))
                    continue
                if "version" not in spec:
                    continue
                newest, repo = crate(name)
                old = spec["version"]
                if old != f"={newest}":
                    notes = f"https://crates.io/crates/{name}/{newest}" + (f"  {repo.rstrip('/')}/releases" if repo else "")
                    changes.append((path, dep, "crate", old, f"={newest}", notes))
    return changes


def apply(changes):
    for path in {c[0] for c in changes}:
        text = path.read_text()
        for _, dep, kind, old, new, _ in (c for c in changes if c[0] == path):
            key = rf'^(\s*"?{re.escape(dep)}"?\s*=\s*)'
            if kind == "crate":
                # `dep = "x"` or `dep = { version = "x", ... }`, on one line.
                plain = re.compile(key + rf'"{re.escape(old)}"', re.M)
                table = re.compile(key + rf'(\{{[^\n]*?version\s*=\s*)"{re.escape(old)}"', re.M)
                text, n = plain.subn(rf'\g<1>"{new}"', text)
                if not n:
                    text, n = table.subn(rf'\g<1>\g<2>"{new}"', text)
            else:
                line = re.compile(key + r"\{[^\n]*\}", re.M)
                def fix(m):
                    s = re.sub(r'tag\s*=\s*"[^"]*"', f'tag = "{new[0]}"', m.group(0))
                    return re.sub(r'rev\s*=\s*"[^"]*"', f'rev = "{new[1]}"', s)
                text, n = line.subn(fix, text)
            if not n:
                sys.exit(f"update-deps: cannot rewrite {dep} in {path}")
        path.write_text(text)


if __name__ == "__main__":
    dry = "--dry-run" in sys.argv
    changes = plan()
    for path, dep, kind, old, new, notes in changes:
        old_s = old if kind == "crate" else f"{old[0]} ({(old[1] or '?')[:10]})"
        new_s = new if kind == "crate" else f"{new[0]} ({new[1][:10]})"
        print(f"{path.relative_to(ROOT)}: {dep} {old_s} -> {new_s}  {notes}")
    if not changes:
        print("nothing to update")
    if not dry:
        apply(changes)
