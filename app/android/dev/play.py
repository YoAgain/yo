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

    class Refused(Exception):
        pass

    def check(response):
        if not response.ok:
            raise Refused(f"{response.request.method} {response.url.split('?')[0]}: {response.status_code} {response.text}")
        return response.json()

    def attempt(status):
        edit = check(session.post(f"{API}/edits"))["id"]
        with open(aab, "rb") as f:
            bundle = check(session.post(f"{UPLOAD}/edits/{edit}/bundles", params={"uploadType": "media"}, data=f,
                                        headers={"Content-Type": "application/octet-stream"}, timeout=900))
        code = bundle["versionCode"]
        release = {"versionCodes": [str(code)], "status": status}
        for track in tracks.split(","):
            check(session.put(f"{API}/edits/{edit}/tracks/{track}", json={"track": track, "releases": [release]}))
        check(session.post(f"{API}/edits/{edit}:commit"))  # Play checks the draft rule here, not at the track
        return code

    try:
        code = attempt(status)
    except Refused as error:
        if status != "completed" or "draft" not in str(error).lower():
            sys.exit(str(error))
        # The app is still a draft in Play: every track takes draft releases only. Start over with a fresh edit.
        status = "draft"
        try:
            code = attempt(status)
        except Refused as again:
            sys.exit(str(again))
    print(f"{PACKAGE} {code} on {tracks} ({status})")


if __name__ == "__main__":
    if len(sys.argv) not in (4, 5):
        sys.exit(__doc__)
    main(*sys.argv[1:])
