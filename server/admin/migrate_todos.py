#!/usr/bin/env python3
"""Run with admin.env loaded and yo-admin stopped. Re-run after a rate limit; progress is saved."""
import json
import os
from pathlib import Path
import urllib.request


def main():
    state = Path(os.environ.get("STATE_DIRECTORY", "./state"))
    source = state / "todos.json"
    owner = source.stat()
    progress = state / "todos-migrated.json"
    done_file = state / "feedback-done.json"
    moved = json.loads(progress.read_text()) if progress.exists() else {}
    done = set(json.loads(done_file.read_text())) if done_file.exists() else set()
    urls = {"live": os.environ.get("YO_HUB_URL", "http://localhost:8080"),
            "dev": os.environ.get("YO_HUB_DEV_URL", "http://localhost:8090")}

    def save(path, value):
        tmp = path.with_suffix(path.suffix + ".new")
        tmp.write_text(json.dumps(value))
        tmp.chmod(0o600)
        if os.geteuid() == 0:
            os.chown(tmp, owner.st_uid, owner.st_gid)
        tmp.replace(path)

    for todo in json.loads(source.read_text()):
        key = f"{todo['hub']}:{todo['id']}"
        if key not in moved:
            request = urllib.request.Request(urls[todo["hub"]].rstrip("/") + "/v1/admin/todos",
                data=todo["text"].encode("utf-8"), headers={"Yo-Admin-Token": os.environ["YO_ADMIN_TOKEN"]})
            with urllib.request.urlopen(request, timeout=10) as response:
                moved[key] = f"{todo['hub']}:{json.load(response)['id']}"
            save(progress, moved)
        if key in done:
            done.remove(key)
            done.add(moved[key])
        save(done_file, sorted(done))
    source.rename(state / "todos.json.migrated")
    print(f"Moved {len(moved)} To-Dos; old file archived.")


if __name__ == "__main__":
    main()
