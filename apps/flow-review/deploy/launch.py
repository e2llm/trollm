#!/usr/bin/env python3
"""Render a rootless Podman pod spec, optionally launch it. Requires PyYAML."""
import argparse
import json
import os
import re
import shlex
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import urlopen

import yaml


def podman_argv():
    return shlex.split(os.environ.get('PODMAN', 'podman'))


def validate_podman_versions(report):
    """Both the CLI and a reported remote runtime must honor memory emptyDir."""
    if not isinstance(report, dict) or 'Client' not in report:
        raise ValueError('Cannot verify Podman Client version')
    versions = {}
    for role in ('Client', 'Server') if 'Server' in report else ('Client',):
        details = report[role]
        version = details.get('Version') if isinstance(details, dict) else None
        match = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)([-+~][A-Za-z0-9.+~-]+)?', version or '') \
            if isinstance(version, str) else None
        if match is None:
            raise ValueError(f'Cannot parse Podman {role} version')
        base = tuple(map(int, match.group(1, 2, 3)))
        prerelease_floor = base == (5, 0, 0) and (match.group(4) or '').startswith(('-', '~'))
        if base < (5, 0, 0) or prerelease_floor:
            raise ValueError(f'Podman {role} {version} is unsupported: require 5.0.0 or newer '
                             'for memory-backed emptyDir volumes')
        versions[role] = version
    return versions


def require_supported_podman():
    try:
        output = subprocess.check_output(podman_argv() + ['version', '--format', 'json'],
                                         text=True, stderr=subprocess.PIPE, timeout=20)
    except subprocess.TimeoutExpired:
        raise SystemExit('Podman version query timed out after 20 seconds; no pod was started') from None
    except (OSError, subprocess.CalledProcessError):
        raise SystemExit('Cannot query Podman runtime versions; require Podman 5.0.0 or newer') from None
    try:
        versions = validate_podman_versions(json.loads(output))
    except (ValueError, TypeError) as error:
        raise SystemExit(str(error) if not isinstance(error, json.JSONDecodeError)
                         else 'Cannot parse Podman version report') from None
    print('Podman versions verified: ' + ', '.join(f'{role}={version}' for role, version in versions.items()))
    return versions


def render(packages, image, port, bind):
    packages = Path(packages).resolve(strict=True)
    if not packages.is_dir():
        raise ValueError('packages must be a directory')
    if not image or any(c.isspace() for c in image):
        raise ValueError('image must be a nonempty image reference')
    if not 1024 <= port <= 65535:
        raise ValueError('rootless port must be between 1024 and 65535')
    spec = yaml.safe_load(Path(__file__).with_name('pod.yaml').read_text())
    spec['spec']['volumes'][0]['hostPath']['path'] = str(packages)
    container = spec['spec']['containers'][0]
    container['image'] = image
    container['ports'][0].update(hostPort=port, hostIP=bind)
    return spec


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--packages', required=True)
    parser.add_argument('--image', required=True)
    parser.add_argument('--port', type=int, default=8080)
    parser.add_argument('--bind', default='127.0.0.1')
    parser.add_argument('--print', action='store_true', dest='print_only')
    args = parser.parse_args()
    spec = render(args.packages, args.image, args.port, args.bind)
    if args.print_only:
        print(json.dumps(spec, indent=2))
        return
    require_supported_podman()
    # JSON is valid Kubernetes YAML. Arguments and paths never enter a shell.
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json') as output:
        json.dump(spec, output)
        output.flush()
        subprocess.run(podman_argv() + ['kube', 'play', '--replace', output.name], check=True)
    # Podman kube play does not implement Kubernetes readinessProbe.
    address = '127.0.0.1' if args.bind == '0.0.0.0' else args.bind
    if ':' in address:
        address = '[' + address + ']'
    for attempt in range(60):
        try:
            with urlopen(f'http://{address}:{args.port}/healthz', timeout=1) as response:
                if response.status == 200:
                    print(f'Ready: http://{address}:{args.port}')
                    return
        except OSError:
            pass
        time.sleep(0.25)
    raise SystemExit('Pod started but readiness failed; inspect podman pod logs trollm-flow-review')


if __name__ == '__main__':
    main()
