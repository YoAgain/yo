#!/usr/bin/env python3
"""Sends one command to the driver (dev/sim.sh drive) and prints its answer.
  drive.py DIR OP [key=value ...]    e.g. drive.py /tmp/d tap label=Start   drive.py /tmp/d tree
Values that look like numbers or true/false are sent as such, [..] as JSON; args=a,b and env=K:V,K2:V2 for launch."""
import json, os, sys, time

d, op, *rest = sys.argv[1:]
cmd = {"op": op, "id": time.time_ns()}
for kv in rest:
    k, v = kv.split("=", 1)
    if k == "args": cmd[k] = v.split(",") if v else []
    elif k == "env": cmd[k] = dict(p.split(":", 1) for p in v.split(",") if p)
    elif v in ("true", "false"): cmd[k] = v == "true"
    elif v.startswith("["): cmd[k] = json.loads(v)
    else:
        try: cmd[k] = int(v) if v.lstrip("-").isdigit() else float(v) if v.replace(".", "", 1).lstrip("-").isdigit() else v
        except ValueError: cmd[k] = v
tmp = os.path.join(d, ".cmd.json")
open(tmp, "w").write(json.dumps(cmd))
os.replace(tmp, os.path.join(d, "cmd.json"))
end = time.time() + float(os.environ.get("DRIVE_TIMEOUT", 60))
while time.time() < end:
    try:
        out = json.load(open(os.path.join(d, "out.json")))
        if out.get("id") == cmd["id"]:
            if "tree" in out: print(out.pop("tree"))
            print(json.dumps(out, ensure_ascii=False))
            sys.exit(0 if out.get("ok") else 1)
    except (OSError, ValueError):
        pass
    time.sleep(0.05)
print("timeout", file=sys.stderr); sys.exit(3)
