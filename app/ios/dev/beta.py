#!/usr/bin/env python3
"""After a TestFlight upload: wait until App Store Connect has processed the newest build, add it to the external
group "Public" (the public link) and submit it to Beta App Review, so outside testers get every update by themselves.

usage: beta.py OAD_TOOLS_DIR [BUNDLE_ID]    (ASC_KEY_ID, ASC_ISSUER_ID, ASC_KEY_PATH as for ship.sh)

A review that cannot be submitted now (another build of the app is still in review) is not an error: the next
upload brings the next try."""
import sys
import time

sys.path.insert(0, sys.argv[1])
import asc  # noqa: E402  (omarchy-apple-dev/tools/asc.py)

BUNDLE = sys.argv[2] if len(sys.argv) > 2 else "app.justyo.ios"
GROUP = "Public"
WAIT_S, EVERY_S = 30 * 60, 30

app = asc.call("GET", f"/v1/apps?filter[bundleId]={BUNDLE}")["data"][0]["id"]
group = next(g["id"] for g in asc.call("GET", f"/v1/apps/{app}/betaGroups")["data"] if g["attributes"]["name"] == GROUP)

deadline = time.time() + WAIT_S
while True:
    builds = asc.call("GET", f"/v1/builds?filter[app]={app}&sort=-uploadedDate&limit=1&include=buildBetaDetail")
    build = builds["data"][0]
    state = build["attributes"]["processingState"]
    external = next((i["attributes"]["externalBuildState"] for i in builds.get("included", [])), None)
    version = build["attributes"]["version"]
    print(f"build {version}: {state}, {external}", flush=True)
    if state == "VALID" and external == "READY_FOR_BETA_SUBMISSION":
        break
    if state in ("FAILED", "INVALID") or external in ("MISSING_EXPORT_COMPLIANCE", "IN_BETA_REVIEW", "BETA_APPROVED",
                                                       "WAITING_FOR_BETA_REVIEW", "IN_BETA_TESTING"):
        print(f"build {version}: nothing to submit ({state}, {external})")
        sys.exit(0)
    if time.time() > deadline:
        print(f"build {version}: still {state} after {WAIT_S // 60} min; the next upload tries again")
        sys.exit(0)
    time.sleep(EVERY_S)

asc.call("POST", f"/v1/betaGroups/{group}/relationships/builds", {"data": [{"type": "builds", "id": build["id"]}]})
print(f"build {version}: in group {GROUP}")
try:
    asc.call("POST", "/v1/betaAppReviewSubmissions",
             {"data": {"type": "betaAppReviewSubmissions",
                       "relationships": {"build": {"data": {"type": "builds", "id": build["id"]}}}}})
    print(f"build {version}: submitted to Beta App Review")
except SystemExit as refused:  # asc.die: e.g. another build of the app still in review
    print(f"build {version}: not submitted now ({refused}); the next upload tries again")
