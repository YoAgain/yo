#!/usr/bin/env python3
"""Inventory resolved runtime Maven POM licenses and embedded notices, without fetching arbitrary URLs."""
import pathlib
import sys
import xml.etree.ElementTree as ET
import zipfile

cache = pathlib.Path(sys.argv[1]) / "caches/modules-2/files-2.1"
coordinates = pathlib.Path(sys.argv[2]).read_text().splitlines()
output = pathlib.Path(sys.argv[3])
output.parent.mkdir(parents=True, exist_ok=True)


def pom(coordinate):
    group, name, version = coordinate.split(":")
    found = sorted((cache / group / name / version).glob("*/*.pom"))
    if not found:
        raise RuntimeError(f"Missing resolved POM: {coordinate}")
    return ET.parse(found[0]).getroot()


def declared(coordinate, seen=None):
    seen = set() if seen is None else seen
    if coordinate in seen or len(seen) > 16:
        raise RuntimeError("Cyclic POM parent")
    seen.add(coordinate)
    root = pom(coordinate)
    licenses = [(row.findtext("{*}name", ""), row.findtext("{*}url", ""))
                for row in root.findall("{*}licenses/{*}license")]
    if licenses:
        return licenses
    parent = root.find("{*}parent")
    if parent is not None:
        parent_id = ":".join(parent.findtext("{*}" + key, "") for key in ("groupId", "artifactId", "version"))
        return declared(parent_id, seen)
    raise RuntimeError(f"No declared license or parent: {coordinate}")


parts = ["Maven runtime dependency inventory\nDeclared licenses, parent POMs and embedded notices.\n"
         "License texts are reproduced below when shipped in the resolved artifact.\n"]
for coordinate in sorted(set(coordinates)):
    parts.append("\n" + coordinate + "\n" + "\n".join(name + " — " + url for name, url in declared(coordinate)))
    group, name, version = coordinate.split(":")
    included = set()
    for archive in sorted((cache / group / name / version).glob("*/*")):
        if archive.suffix not in (".jar", ".aar") or "sources" in archive.name or "javadoc" in archive.name:
            continue
        with zipfile.ZipFile(archive) as zipped:
            for entry in sorted(zipped.namelist()):
                base = pathlib.PurePosixPath(entry).name.upper()
                if not base.startswith(("LICENSE", "NOTICE", "COPYING")) or entry.endswith("/"):
                    continue
                data = zipped.read(entry)
                if data in included:
                    continue
                included.add(data)
                parts.append("\n" + entry + "\n" + data.decode("utf-8", errors="replace"))
output.write_text("\n".join(parts))
