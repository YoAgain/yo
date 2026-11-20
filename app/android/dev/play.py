#!/usr/bin/env python3
"""Uploads an Android App Bundle to Google Play and puts it on a test track (Play Developer API, one edit).

    play.py SERVICE_ACCOUNT_JSON AAB TRACKS [STATUS]

TRACKS, comma separated: internal (every push to main), alpha (closed testing, published releases too). STATUS is completed unless
given; while Play still shows the app as a draft it takes only draft releases, so a refused completed release goes up as draft.
Needs google-auth and requests.
"""
import sys

from google.auth.transport.requests import AuthorizedSession
from google.oauth2 import service_account

PACKAGE = "app.justyo.android"
API = f"https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{PACKAGE}"
UPLOAD = f"https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/{PACKAGE}"


def main(key, aab, tracks, status="completed"):
    credentials = service_account.Credentials.from_service_account_file(
        key, scopes=["https://www.googleapis.com/auth/androidpublisher"])
    session = AuthorizedSession(credentials)

    def check(response):
        if not response.ok:
            sys.exit(f"{response.request.method} {response.url.split('?')[0]}: {response.status_code} {response.text}")
        return response.json()

    edit = check(session.post(f"{API}/edits"))["id"]
    with open(aab, "rb") as f:
        bundle = check(session.post(f"{UPLOAD}/edits/{edit}/bundles", params={"uploadType": "media"}, data=f,
                                    headers={"Content-Type": "application/octet-stream"}, timeout=900))
    code = bundle["versionCode"]
    release = {"versionCodes": [str(code)], "status": status}
    for track in tracks.split(","):
        response = session.put(f"{API}/edits/{edit}/tracks/{track}", json={"track": track, "releases": [release]})
        if not response.ok and status == "completed" and "draft" in response.text.lower():
            status = release["status"] = "draft"  # the app is still a draft in Play: draft releases only
            response = session.put(f"{API}/edits/{edit}/tracks/{track}", json={"track": track, "releases": [release]})
        check(response)
    check(session.post(f"{API}/edits/{edit}:commit"))
    print(f"{PACKAGE} {code} on {tracks} ({status})")


if __name__ == "__main__":
    if len(sys.argv) not in (4, 5):
        sys.exit(__doc__)
    main(*sys.argv[1:])
