#!/usr/bin/env python3
"""Install and launch the management-only app in an existing iOS 26 runtime."""
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import time


def run(*args, timeout=60):
    print('$', ' '.join(map(str, args)), flush=True)
    started = time.monotonic()
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        for partial in (error.stdout, error.stderr):
            if partial:
                print(partial.decode(errors='replace') if isinstance(partial, bytes) else partial,
                      flush=True)
        print(f'Timed out after {time.monotonic() - started:.1f}s (budget {timeout}s)', flush=True)
        raise
    print(result.stdout, result.stderr, f'Elapsed: {time.monotonic() - started:.1f}s', flush=True)
    result.check_returncode()
    return result.stdout.strip()


def select_runtime(runtimes):
    eligible = [r for r in runtimes if r.get('isAvailable')
                and r.get('version', '').split('.')[0] == '26'
                and '.iOS-' in r.get('identifier', '')]
    if not eligible:
        raise ValueError('No installed available iOS 26 runtime; no download attempted')
    return max(eligible, key=lambda r: tuple(map(int, r['version'].split('.'))))['identifier']


def simulator_metadata(info):
    info = dict(info)
    if not info.get('UIApplicationSceneManifest') or info.get('MinimumOSVersion') != '26.0':
        raise ValueError('Missing scene configuration or iOS 26 minimum')
    info['CFBundleSupportedPlatforms'] = ['iPhoneSimulator']
    return info


def launch_app(udid, bundle_id, phase, output, report):
    def launch(prefix, retry=False):
        args = ['xcrun', 'simctl', 'launch']
        if retry:
            args.append('--terminate-running-process')
        return run(*args, f'--stdout={output / (prefix + ".stdout.log")}',
                   f'--stderr={output / (prefix + ".stderr.log")}', udid, bundle_id)

    try:
        return launch(phase), phase
    except subprocess.TimeoutExpired as error:
        partial = '\n'.join(value.decode(errors='replace') if isinstance(value, bytes) else value
                            for value in (error.stdout, error.stderr) if value)
        app_output = any((output / (phase + suffix)).exists()
                         and (output / (phase + suffix)).stat().st_size
                         for suffix in ('.stdout.log', '.stderr.log'))
        # Only retry a first launch with no PID or app output, never an app failure.
        if (phase != 'first-launch' or re.search(r':\s*\d+\s*$', partial, re.MULTILINE)
                or 'XSCC_GPUI_' in partial or app_output):
            raise
        report['first_attempt_timed_out'] = True
        report['retried'] = True
        report['first_attempt_timeout_seconds'] = error.timeout
        prefix = phase + '-retry'
        return launch(prefix, retry=True), prefix


def smoke(binary, output):
    output.mkdir(parents=True, exist_ok=True)
    udid = None
    report = {'scope': 'Management-only Simulator smoke; no codec/session or touch automation',
              'result': 'failed', 'not_verified': ['physical device', 'remote streaming', 'touch interaction',
                                                  'visual correctness (inspect screenshots)']}
    try:
        build = run('xcrun', 'vtool', '-show-build', str(binary))
        if not re.search(r'platform\s+IOSSIMULATOR\s', build + '\n') or not re.search(r'minos\s+26\.0(?:\.0)?\s', build + '\n'):
            raise ValueError('Expected iOS Simulator minimum 26.0 executable')
        if run('xcrun', 'lipo', '-archs', str(binary)) != 'arm64':
            raise ValueError('Expected ARM64 simulator executable')
        template = Path(__file__).resolve().parents[2] / 'packaging/gpui-ios/Info.plist'
        info = simulator_metadata(plistlib.loads(template.read_bytes()))
        app = output / 'XSCCSimulator.app'
        app.mkdir()
        shutil.copy2(binary, app / info['CFBundleExecutable'])
        (app / 'Info.plist').write_bytes(plistlib.dumps(info))
        run('codesign', '--force', '--sign', '-', str(app))
        runtimes = json.loads(run('xcrun', 'simctl', 'list', 'runtimes', '-j'))
        runtime = select_runtime(runtimes['runtimes'])
        types = json.loads(run('xcrun', 'simctl', 'list', 'devicetypes', '-j'))['devicetypes']
        iphones = [d for d in types if d['name'].startswith('iPhone')]
        device = next((d for d in iphones if d['name'] == 'iPhone 16'), iphones[-1])
        udid = run('xcrun', 'simctl', 'create', 'XSCC management smoke', device['identifier'], runtime)
        report.update(runtime=runtime, device=device['name'], udid=udid)
        run('xcrun', 'simctl', 'boot', udid)
        run('xcrun', 'simctl', 'bootstatus', udid, '-b', timeout=600)
        run('xcrun', 'simctl', 'install', udid, str(app), timeout=120)
        for phase in ('first-launch', 'relaunch'):
            launch, log_prefix = launch_app(udid, info['CFBundleIdentifier'], phase, output, report)
            pid = int(launch.rsplit(':', 1)[1].strip())
            time.sleep(8)
            run('xcrun', 'simctl', 'io', udid, 'screenshot', str(output / (phase + '.png')))
            os.kill(pid, 0)
            stderr = (output / (log_prefix + '.stderr.log')).read_text()
            for marker in ('XSCC_GPUI_LAUNCH_CALLBACK_REACHED', 'XSCC_GPUI_WINDOW_CREATED'):
                if marker not in stderr:
                    raise ValueError(f'{phase}: missing lifecycle marker {marker}')
            report[phase] = {'pid': pid, 'alive_after_seconds': 8, 'screenshot': phase + '.png',
                             'log_prefix': log_prefix}
            run('xcrun', 'simctl', 'terminate', udid, info['CFBundleIdentifier'])
        report['result'] = 'installed_and_launched_twice; screenshots_require_review'
    except Exception as error:
        if isinstance(error, subprocess.TimeoutExpired):
            report['timeout_command'] = list(error.cmd)
            report['timeout_budget_seconds'] = error.timeout
        report['error'] = str(error)
        raise
    finally:
        cleanup_errors = {}
        if udid:
            for args in [('io', udid, 'screenshot', str(output / 'final-state.png')),
                         ('spawn', udid, 'log', 'show', '--last', '15m', '--style', 'compact',
                          '--predicate', 'process == "xscc-gpui-mobile-management" OR process == "SpringBoard" OR process == "backboardd" OR process == "launchd_sim"'),
                         ('shutdown', udid), ('delete', udid)]:
                try:
                    text = run('xcrun', 'simctl', *args)
                    if args[0] == 'spawn':
                        (output / 'process-system.log').write_text(text)
                except Exception as error:
                    print('Diagnostic/cleanup:', error, flush=True)
                    if args[0] in ('shutdown', 'delete'):
                        cleanup_errors[args[0]] = str(error)
        report['cleanup_errors'] = cleanup_errors
        if 'delete' in cleanup_errors:
            report['result'] = 'failed_temporary_simulator_cleanup'
        (output / 'smoke-result.json').write_text(json.dumps(report, indent=2) + '\n')
        # Only diagnostics/screenshots are artifacts, never a simulator installation package.
        shutil.rmtree(output / 'XSCCSimulator.app', ignore_errors=True)
        if 'delete' in cleanup_errors:
            raise RuntimeError('Could not delete the temporary simulator: ' + udid)


if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit('Usage: smoke-gpui-ios-simulator.py SIMULATOR_EXECUTABLE OUTPUT_DIRECTORY')
    smoke(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
