"""Resumable real-device runs. Only YO_PERF_RUN records are retained, never the device's other logs.
Physical taps/scroll and warm background/foreground need an operator (no touch injection API).
"""
import argparse
import asyncio
import importlib.util
import json
import os
import re
import time
from pathlib import Path
import sys
import uuid

# -I intentionally excludes the script directory: load only our exact sibling files.
def sibling(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

wireless = sibling('wireless')
parser = sibling('perf_parse')
# Yos per sender: direct and group 6 x 17 = 102 (about 3 s), six-senders 6 x 20.
PER_SENDER = {'direct': 17, 'group': 17, 'six-senders': 20, 'photo': 1}
SCENARIOS = ('direct', 'group', 'six-senders', 'photo', 'cold', 'warm', 'tap30')
# PERF_SCENARIOS=direct,group,... runs only those (unattended: leave out warm and tap30, they need an operator).
if os.environ.get('PERF_SCENARIOS'):
    SCENARIOS = tuple(s for s in os.environ['PERF_SCENARIOS'].split(',') if s in SCENARIOS)


async def main(args):
    from pymobiledevice3.remote import tunnel_service
    from pymobiledevice3.remote.core_device.app_service import AppServiceService
    from pymobiledevice3.remote.remote_service_discovery import RemoteServiceDiscoveryService
    from pymobiledevice3.remote.userspace_tunnel import UserspaceDialPlane
    from pymobiledevice3.services.os_trace import OsTraceService

    output = Path('target/perf/runs.jsonl')
    # A killed append may leave one incomplete final line. Never discard a complete record.
    if output.exists():
        raw = output.read_bytes()
        if raw and not raw.endswith(b'\n'):
            output.write_bytes(raw[:raw.rfind(b'\n') + 1])
    completed = {int(r['run']) for r in parser.records(output)}
    state_dir = Path('target/perf-state')
    state_dir.mkdir(parents=True, exist_ok=True)
    svc = await wireless.find_service(wireless.pick_udid(None), args.host, None)
    tunnel_service.USE_USERSPACE_TUNNEL = True
    try:
        async with svc.start_tcp_tunnel() as t:
            t.client.tun.set_peer(t.address)
            async with UserspaceDialPlane(t.client.tun, t.address) as dial:
                rsd = RemoteServiceDiscoveryService((t.address, t.port), open_connection=dial.dial,
                                                   auxiliary_metadata=t.auxiliary_metadata)
                await rsd.connect()
                try:
                    await wireless.mount(rsd)
                    await rsd.close()
                    rsd = RemoteServiceDiscoveryService((t.address, t.port), open_connection=dial.dial,
                                                       auxiliary_metadata=t.auxiliary_metadata)
                    await rsd.connect()
                    pending = {}
                    log_ready = asyncio.Event()
                    async def logs():
                        async with OsTraceService(rsd) as service:
                            log_ready.set()
                            async for entry in service.syslog():
                                line = entry.message
                                if isinstance(line, str) and 'YO_PERF_RUN {' in line:
                                    raw = line.split('YO_PERF_RUN ', 1)[1]
                                    try:
                                        row, _ = json.JSONDecoder().raw_decode(raw)
                                    except ValueError:
                                        continue
                                    future = pending.get(row.get('run'))
                                    if future and not future.done():
                                        future.set_result(row)
                    log_task = asyncio.create_task(logs())
                    await asyncio.wait_for(log_ready.wait(), 10)
                    await asyncio.sleep(1)
                    if log_task.done():
                        await log_task
                    async def send(sender, scenario):
                        command = ['target/release/yo-send', '--url', 'https://dev.justyo.app',
                                   '--state', str(state_dir / f'{args.prefix}-{sender}'),
                                   '--name', f'{args.prefix}{sender}', '--to', args.group if scenario == 'group' else args.to,
                                   '--count', str(PER_SENDER[scenario]), '--every-ms', '0']
                        if scenario == 'group':
                            command += ['--group', '--sync']
                        if scenario == 'photo':
                            command[command.index('--count') + 1] = '1'
                            command += ['--photo', args.photo]
                        # No account names or transport logs in result files.
                        process = await asyncio.create_subprocess_exec(*command, stdout=asyncio.subprocess.PIPE,
                                                                       stderr=asyncio.subprocess.DEVNULL)
                        try:
                            stdout, _ = await asyncio.wait_for(process.communicate(), 120)
                            code = process.returncode
                        except BaseException:
                            process.kill()
                            await process.wait()
                            raise
                        lines = stdout.decode().splitlines()
                        messages = [line for line in lines if line.startswith(('yo ', 'media at '))]
                        expected = PER_SENDER[scenario]
                        # A few refused by the hub's limits (they stay in the sender's outbox) still make a burst.
                        sent = sum(line.endswith('true') for line in messages)
                        if len(messages) != expected or sent < 0.9 * expected:
                            raise RuntimeError(f'yo-send delivered {sent} of {expected}; inspect test account/group setup')
                        stamps = [float(re.search(r'at ([0-9.]+)', line)[1]) for line in messages]
                        span = max(stamps) - min(stamps)
                        return min(stamps), max(stamps)
                    try:
                        async with AppServiceService(rsd) as apps:
                            for index in range(args.runs):
                                if index in completed:
                                    continue
                                scenario = SCENARIOS[index % len(SCENARIOS)]
                                token = f'{index}-{uuid.uuid4().hex}'
                                future = asyncio.get_running_loop().create_future()
                                pending[token] = future
                                print(f'Run {index + 1}/{args.runs}: {scenario}', flush=True)
                                await apps.launch_application('app.justyo.ios.dev', environment={
                                    'YO_PERF': '1', 'YO_PERF_RUN': token, 'YO_PERF_SCENARIO': scenario,
                                    'YO_PERF_SECONDS': '15'})
                                load_start = time.monotonic()
                                if scenario in ('tap30', 'warm'):
                                    print('NOW: tap a test friend 30 times in 15 s.' if scenario == 'tap30' else
                                          'NOW: go Home, then reopen Yo (warm) within 15 s.', flush=True)
                                elif scenario in ('direct', 'group', 'six-senders', 'photo'):
                                    await asyncio.sleep(1)
                                    # One sender manages ~6 Yos/s (a hub round trip each): 100 in 3 s takes six at once.
                                    spans = await asyncio.gather(*(send(n, scenario) for n in
                                                                 ([0] if scenario == 'photo' else range(6))))
                                    load_ms = max(b for _, b in spans) - min(a for a, _ in spans)
                                    print(f'  load: {sum(PER_SENDER[scenario] for _ in spans)} Yos in {load_ms:.0f} ms', flush=True)
                                    if time.monotonic() - load_start > 14:
                                        raise RuntimeError('Load exceeded measurement window; run not saved')
                                row = await asyncio.wait_for(asyncio.shield(future), 45)
                                if scenario in ('tap30', 'warm'):
                                    answer = await asyncio.to_thread(input, 'Completed the physical action correctly? [y/N] ')
                                    if answer.lower() != 'y':
                                        raise RuntimeError('Run not saved; rerun to resume')
                                row['run'] = str(index)
                                with output.open('a') as stream:
                                    stream.write(json.dumps(row, sort_keys=True) + '\n')
                                    stream.flush()
                                    os.fsync(stream.fileno())
                                pending.pop(token)
                                parser.summarize(output)
                                # The hub takes 1000 deliveries a minute from one address; a group run alone is
                                # ~600 (each Yo to every member): let the minute pass before the next load.
                                if scenario in ('group', 'six-senders'):
                                    await asyncio.sleep(45)
                    finally:
                        log_task.cancel()
                        await asyncio.gather(log_task, return_exceptions=True)
                finally:
                    await rsd.close()
    finally:
        await svc.close()


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('host')
    for key in ('to', 'group', 'prefix', 'photo'):
        ap.add_argument('--' + key, required=True)
    ap.add_argument('--runs', type=int, default=300)
    asyncio.run(main(ap.parse_args()))
