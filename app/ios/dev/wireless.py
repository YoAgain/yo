#!/usr/bin/env python3
"""Install and launch a signed .ipa on an iPhone over the network, without root and without a cable.

  wireless.py HOST IPA BUNDLE_ID [--udid UDID] [--port PORT]
  wireless.py HOST --shot FILE.png        a screenshot of the phone instead

HOST is the phone's IP address (Tailscale or Wi-Fi). Needs iOS 27, a RemotePairing record made once over USB
(omarchy-apple-dev: tools/tailnet-tunnel.py pair, in ~/.local/share/pymobiledevice3/) and pymobiledevice3's
Python. The tunnel runs on pymobiledevice3's userspace TCP stack inside this process, so no TUN device and no
sudo: every service (install, disk image, launch) is reached through that one in-process tunnel.
"""
import argparse
import asyncio
import sys
from pathlib import Path

PORT_SCAN = range(49152, 49168)
RECORD_DIR = Path.home() / ".local/share/pymobiledevice3"


def pick_udid(udid):
    if udid:
        return udid
    found = [p.stem.removeprefix("remote_") for p in sorted(RECORD_DIR.glob("remote_*.plist"))]
    if len(found) != 1:
        sys.exit(f"need --udid: {len(found)} RemotePairing records in {RECORD_DIR}")
    return found[0]


async def find_service(udid, host, port):
    from pymobiledevice3.remote.tunnel_service import RemotePairingTunnelService

    last = None
    for p in [port] if port else PORT_SCAN:
        svc = RemotePairingTunnelService(udid, host, p)
        try:
            await asyncio.wait_for(svc.connect(autopair=False), timeout=20)
            return svc
        except (OSError, TimeoutError, asyncio.TimeoutError) as e:
            last = e
    sys.exit(f"no RemotePairing service answered on {host} (Wi-Fi and the VPN on, phone unlocked?): {last!r}")


async def run(args):
    import pymobiledevice3.remote.tunnel_service as tunnel_service
    from pymobiledevice3.remote.core_device.app_service import AppServiceService
    from pymobiledevice3.remote.remote_service_discovery import RemoteServiceDiscoveryService
    from pymobiledevice3.remote.userspace_tunnel import UserspaceDialPlane
    from pymobiledevice3.services.installation_proxy import InstallationProxyService
    from pymobiledevice3.exceptions import CoreDeviceError

    svc = await find_service(pick_udid(args.udid), args.host, args.port)
    tunnel_service.USE_USERSPACE_TUNNEL = True
    try:
        async with svc.start_tcp_tunnel() as t:
            t.client.tun.set_peer(t.address)
            async with UserspaceDialPlane(t.client.tun, t.address) as dial:
                rsd = RemoteServiceDiscoveryService(
                    (t.address, t.port), open_connection=dial.dial, auxiliary_metadata=t.auxiliary_metadata
                )
                await rsd.connect()
                try:
                    print(f"== tunnel up (no root): {args.host} -> {t.address}", flush=True)
                    if args.shot:
                        await mount(rsd)
                        # The RSD lists the services it saw at connect; the image's (screenshots) came after.
                        await rsd.close()
                        rsd = RemoteServiceDiscoveryService(
                            (t.address, t.port), open_connection=dial.dial, auxiliary_metadata=t.auxiliary_metadata
                        )
                        await rsd.connect()
                        await screenshot(rsd, args.shot)
                        return
                    await InstallationProxyService(lockdown=rsd).install_from_local(Path(args.ipa))
                    print("== installed", flush=True)
                    await mount(rsd)
                    # The RSD lists the services it saw at connect; the image's services (launching) came after.
                    await rsd.close()
                    rsd = RemoteServiceDiscoveryService(
                        (t.address, t.port), open_connection=dial.dial, auxiliary_metadata=t.auxiliary_metadata
                    )
                    await rsd.connect()
                    try:
                        async with AppServiceService(rsd) as apps:
                            await apps.launch_application(args.bundle_id)
                        print(f"== launched {args.bundle_id}", flush=True)
                    except CoreDeviceError as e:
                        # 10002 when the phone is locked with its screen off: installed all the same.
                        print(f"== installed, not launched (phone locked?): {e}", flush=True)
                finally:
                    await rsd.close()
    finally:
        await svc.close()


async def mount(rsd):
    """The developer disk image: it carries launching and screenshots; nothing to do once mounted."""
    from pymobiledevice3.exceptions import AlreadyMountedError
    from pymobiledevice3.services.mobile_image_mounter import auto_mount

    try:
        await auto_mount(rsd)
    except AlreadyMountedError:
        pass


async def screenshot(rsd, path):
    from pymobiledevice3.services.dvt.instruments.dvt_provider import DvtProvider
    from pymobiledevice3.services.dvt.instruments.screenshot import Screenshot

    async with DvtProvider(rsd) as provider, Screenshot(provider) as shot:
        Path(path).write_bytes(await shot.get_screenshot())
    print(f"== screenshot: {path}", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("host")
    ap.add_argument("ipa", nargs="?")
    ap.add_argument("bundle_id", nargs="?")
    ap.add_argument("--shot", help="write a screenshot of the phone to this PNG file and do nothing else")
    ap.add_argument("--udid")
    ap.add_argument("--port", type=int, help="RemotePairing port (default: scan 49152-49167)")
    asyncio.run(run(ap.parse_args()))


if __name__ == "__main__":
    main()
