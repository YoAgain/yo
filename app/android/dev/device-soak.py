#!/usr/bin/env python3
"""Run the safe A34 UI checks against an isolated debug package on the DEV hub."""

import argparse
import json
from pathlib import Path
import re
import secrets
import subprocess
import time
import xml.etree.ElementTree as ET

PACKAGE = "app.justyo.android.dev.soak"
ACTIVITY = f"{PACKAGE}/app.justyo.android.DefaultIcon"


class CheckFailed(Exception):
    pass


class Phone:
    def __init__(self):
        self.serial = None
        self.wait_for_usb()

    def wait_for_usb(self):
        deadline = time.monotonic() + 300
        while True:
            try:
                output = subprocess.check_output(
                    ["adb", "devices"], text=True, timeout=10, stderr=subprocess.DEVNULL
                )
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                output = ""
            online = [
                parts[0] for line in output.splitlines()
                if len(parts := line.split()) == 2 and parts[1] == "device"
                and not parts[0].startswith("emulator-")
            ]
            if self.serial in online:
                return
            if self.serial is None and len(online) == 1:
                self.serial = online[0]
                return
            if time.monotonic() >= deadline:
                raise CheckFailed("The selected USB phone was unavailable for five minutes")
            time.sleep(10)

    def adb(self, *args, timeout=25):
        try:
            result = subprocess.run(
                ["adb", "-s", self.serial, *args], capture_output=True, timeout=timeout
            )
        except subprocess.TimeoutExpired:
            raise CheckFailed("ADB timed out before the step could be confirmed") from None
        if result.returncode:
            if any(error in result.stderr for error in (b"not found", b"device offline", b"no devices")):
                self.wait_for_usb()
            raise CheckFailed("ADB command failed before the step could be confirmed")
        return result.stdout

    def shell(self, *args):
        return self.adb("shell", *args)

    def input(self, *args):
        self.shell("input", *args)
        time.sleep(0.4)

    def nodes(self):
        document = self.adb("exec-out", "uiautomator", "dump", "/dev/tty")
        start, end = document.find(b"<?xml"), document.rfind(b"</hierarchy>")
        if start < 0 or end < 0:
            return []
        return list(ET.fromstring(document[start:end + len(b"</hierarchy>")]).iter("node"))

    def require_qa(self):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if any(node.get("package") == PACKAGE for node in self.nodes()):
                return
        raise CheckFailed("The isolated QA app is not the active window")

    def locate(self, label, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            entries = self.nodes()
            for node in entries:
                if label in (node.get("text"), node.get("content-desc")):
                    return node
            for node in entries:
                if any(node.get(key, "").startswith(label) for key in ("text", "content-desc")):
                    return node
        raise CheckFailed(f"Expected control not visible: {label}")

    def tap_node(self, node):
        x1, y1, x2, y2 = map(int, re.findall(r"\d+", node.get("bounds", "")))
        self.input("tap", str((x1 + x2) // 2), str((y1 + y2) // 2))

    def tap(self, label):
        self.tap_node(self.locate(label))

    def edit(self, value, *, clear=False):
        field = next((node for node in self.nodes() if node.get("class") == "android.widget.EditText"), None)
        if field is None:
            raise CheckFailed("Expected an editable text field")
        self.tap_node(field)
        if clear:
            self.input("keyevent", "KEYCODE_MOVE_END")
            self.shell("input", "keyevent", *(["67"] * len(field.get("text", ""))))
        self.input("text", value)

    def shot(self, path):
        image = self.adb("exec-out", "screencap", "-p")
        if not image.startswith(b"\x89PNG\r\n\x1a\n"):
            raise CheckFailed("Screenshot was not a PNG")
        path.write_bytes(image)


class Run:
    def __init__(self, phone, directory, *, resume=False):
        self.phone = phone
        self.directory = directory
        if resume:
            self.checked = json.loads((directory / "result.json").read_text())["observed"]
        else:
            self.directory.mkdir(parents=True, exist_ok=False)
            self.checked = []

    def save_result(self, *, passed=False, failure=None):
        result = {
            "registration_attempted": "registration-submitted" in self.checked,
            "registration_confirmed": "friend-list" in self.checked,
            "observed": self.checked,
            "canary_passed": passed,
            "complete_user_run": False,
            "failure": failure,
        }
        temporary = self.directory / "result.tmp"
        temporary.write_text(json.dumps(result, indent=2) + "\n")
        temporary.replace(self.directory / "result.json")

    def screen(self, key, label):
        self.phone.locate(label)
        self.phone.shot(self.directory / f"{key}.png")
        self.checked.append(key)
        self.save_result()

    def visit(self, key, entry, visible):
        self.phone.tap(entry)
        self.screen(key, visible)

    def reset(self):
        result = self.phone.shell("pm", "clear", PACKAGE).decode()
        if "Success" not in result:
            raise CheckFailed("Could not reset the isolated QA package")
        self.phone.shell("am", "start", "-W", "-n", ACTIVITY)

    def recovery(self):
        self.screen("welcome", "Sign Up")
        self.visit("sign-in", "Got Yo?", "Use Passkey")
        self.visit("recovery-key", "Use Recovery Key", "Recovery Key")
        self.phone.tap("64 characters")
        self.phone.input("text", "invalidkey")
        self.screen("recovery-typing", "invalidkey")
        self.phone.tap("Sign In")
        self.screen("recovery-error", "That doesn’t match a recovery key")

    def onboarding(self, account):
        self.phone.shell("am", "force-stop", PACKAGE)
        self.phone.shell("am", "start", "-W", "-n", f"{PACKAGE}/app.justyo.android.MainActivity")
        self.phone.require_qa()
        self.phone.tap("Sign Up")
        self.phone.locate("Username")
        self.phone.input("text", "a-b")
        self.screen("registration-invalid", "ab")
        self.phone.edit("andtest99999999999", clear=True)
        self.screen("registration-too-long", "andtest999999999")
        self.phone.edit("PING", clear=True)
        self.screen("registration-taken", "Name taken. Yours?")
        self.phone.edit(account, clear=True)
        self.screen("registration-typing", account)
        self.phone.tap("Sign Up")
        self.checked.append("registration-submitted")
        self.save_result()

    def signed_in(self, account):
        for label in ("Erlauben", "Allow"):
            node = next((node for node in self.phone.nodes() if node.get("text") == label), None)
            if node is not None:
                self.screen("notification-permission", label)
                self.phone.tap(label)
                break
        self.phone.require_qa()
        self.screen("friend-list", "YoGPT")
        self.phone.locate("Group")
        self.phone.locate("Invite")
        self.visit("add-friend", "+", "Name")
        unknown = "NoSuchYoQa" + str(secrets.randbelow(100000))
        self.phone.input("text", unknown)
        self.screen("add-friend-typing", unknown)
        self.phone.input("keyevent", "66")
        self.screen("add-friend-missing", "No Such User")
        self.visit("group-form", "Group", "Group Name")
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.visit("settings", "Settings", "App Icon")
        self.visit("help", "Help & About", "Contact Us")
        self.visit("status", "Status", "Round Trip")
        self.visit("encryption", "Encryption", "Post-Quantum")
        self.phone.tap("Back")
        self.phone.tap("Back")
        self.visit("licenses", "About & Licenses", "Source")
        self.phone.tap("Back")
        self.phone.tap("Back")
        self.screen("profile-for-account", "App Icon")
        self.visit("account", account, "Account Recovery")
        self.visit("account-recovery", "Account Recovery", "Account Backup")
        self.visit("account-backup", "Account Backup", "Account Recovery")
        self.visit("recovery-guide", "How Account Recovery Works", "Done")
        self.phone.tap("Done")
        self.phone.tap("Back")
        self.visit("delete-form", "Leaving Yo for good?", f"Type {account} to confirm.")
        self.phone.edit(account)
        self.screen("delete-typed", account)
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.phone.tap("Back")
        self.visit("contacts", "Contacts", "YoGPT")
        self.visit("contact-sheet", "YoGPT", "Undo Before Sending")
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.phone.tap("Back")
        self.phone.tap("Back")
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.camera()

    def settings_dark_extras(self, account, check, open_qa):
        routes = (
            ("appearance", ("App Icon",), "Appearance"),
            ("blocked", (account, "Blocked"), "No Blocked Friends"),
            ("media-settings", (account, "Media"), "Keep Sent Photos"),
            ("statistics", (account, "Statistics"), "Yos Sent"),
            ("contact-us", ("Help & About", "Contact Us"), "Yo Team"),
            ("crash-reports", ("Help & About", "Status", "Crash Reports"), "Crash Reports"),
        )

        def visit_route(route, key, visible):
            open_qa()
            self.phone.tap("Settings")
            for label in route:
                self.phone.tap(label)
            self.screen(key, visible)

        for key, route, visible in routes:
            check(key, lambda: visit_route(route, key, visible))

    def camera(self):
        self.phone.require_qa()
        self.phone.tap("Camera")
        nodes = self.phone.nodes()
        for label in ("Bei Nutzung der App", "While using the app"):
            if not any(node.get("text") == label for node in nodes):
                continue
            self.screen("camera-permission", label)
            self.phone.tap(label)
            for deny in ("Nicht erlauben", "Don't allow"):
                if any(node.get("text") == deny for node in self.phone.nodes()):
                    self.screen("microphone-permission", deny)
                    self.phone.tap(deny)
                    break
            break
        self.phone.require_qa()
        self.screen("camera-preview", "Close Camera")
        self.screen("camera-hint", "Tap or hold a name")
        self.visit("camera-flipped", "Switch Camera", "Close Camera")
        self.phone.tap("Close Camera")

    def invite(self):
        self.phone.require_qa()
        self.phone.tap("+")
        self.phone.tap("Invite")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if any(
                node.get("package") in {"android", "com.android.intentresolver"}
                for node in self.phone.nodes()
            ):
                break
        else:
            raise CheckFailed("The Android invite sharesheet did not appear")
        self.phone.shot(self.directory / "invite-sharesheet.png")
        self.checked.append("invite-sharesheet")
        self.save_result()
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.phone.require_qa()

    def ping_group(self):
        self.phone.require_qa()
        if not any(node.get("text") == "PING" for node in self.phone.nodes()):
            self.phone.tap("+")
            self.phone.edit("PING")
            self.phone.input("keyevent", "66")
        self.screen("ping-added", "PING")
        if any(node.get("text", "").startswith("#DeviceSoak") for node in self.phone.nodes()):
            self.screen("group-created", "#DeviceSoak")
            return
        self.phone.tap("+")
        self.visit("group-create-form", "Group", "Group Name")
        self.phone.tap("Group Name")
        self.phone.input("text", "DeviceSoak")
        self.screen("group-name-typed", "DeviceSoak")
        self.phone.input("keyevent", "KEYCODE_BACK")
        self.phone.tap("PING")
        self.screen("group-member", "Members · 2")
        self.phone.tap("Create")
        self.screen("group-created", "#DeviceSoak")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dark-first", action="store_true")
    resume = parser.add_mutually_exclusive_group()
    resume.add_argument("--resume-camera", type=Path, metavar="RUN_DIR")
    resume.add_argument("--resume-group", type=Path, metavar="RUN_DIR")
    args = parser.parse_args()
    if not 1 <= args.runs <= 250:
        parser.error("the first requested batch is at most 250 runs")
    if args.runs != 1 and (args.resume_camera or args.resume_group):
        parser.error("resume is for a single existing run")
    args.output.mkdir(parents=True, exist_ok=True)
    phone = Phone()
    if not phone.shell("pm", "path", PACKAGE).startswith(b"package:"):
        raise CheckFailed("Install the isolated yoDeviceSoak debug APK first")
    phone.shell("svc", "power", "stayon", "usb")
    resume_dir = args.resume_camera or args.resume_group
    failures = 0
    for index in range(1, args.runs + 1):
        if index > 1:
            time.sleep(5)
        attempt = Run(phone, resume_dir or args.output / f"run-{time.time_ns()}", resume=resume_dir is not None)
        account = f"andtest{secrets.randbelow(10**9):09d}"
        failed_steps = []

        def check(name, action):
            try:
                action()
                return True
            except CheckFailed as failure:
                failed_steps.append(f"{name}: {failure}")
                try:
                    phone.require_qa()
                    phone.shot(attempt.directory / f"failure-{len(failed_steps):02d}.png")
                except CheckFailed:
                    pass
                print(f"[{index}/{args.runs}] {failed_steps[-1]}", flush=True)
                return False

        def open_qa():
            phone.shell("am", "force-stop", PACKAGE)
            phone.shell("am", "start", "-W", "-n", f"{PACKAGE}/app.justyo.android.MainActivity")
            phone.require_qa()

        print(f"[{index}/{args.runs}] {attempt.directory.name}", flush=True)
        attempt.save_result()
        night = None
        if args.dark_first and index == 1:
            status = phone.shell("cmd", "uimode", "night").decode().strip()
            match = re.search(r"Night mode: (yes|no|auto|custom_bedtime|custom_schedule)$", status)
            if match:
                night = match[1]
                phone.shell("cmd", "uimode", "night", "yes")
        if resume_dir:
            checkpoint = "contact-sheet" if args.resume_camera else "camera-flipped"
            if checkpoint not in attempt.checked:
                raise CheckFailed(f"Resume requires the {checkpoint} checkpoint")
        elif check("reset", attempt.reset):
            check("recovery", attempt.recovery)
            if check("registration", lambda: attempt.onboarding(account)):
                check("signed-in pages", lambda: attempt.signed_in(account))

        if "friend-list" in attempt.checked:
            if "camera-flipped" not in attempt.checked and check("open camera stage", open_qa):
                check("camera", attempt.camera)
            if night is not None:
                attempt.settings_dark_extras(account, check, open_qa)
            if check("open group stage", open_qa):
                check("PING and group", attempt.ping_group)
            if check("open invite stage", open_qa):
                check("invite", attempt.invite)
        attempt.save_result(passed=not failed_steps, failure="; ".join(failed_steps) or None)
        failures += bool(failed_steps)
        if night is not None:
            phone.shell("cmd", "uimode", "night", night)
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
