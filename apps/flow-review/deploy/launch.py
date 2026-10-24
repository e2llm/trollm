#!/usr/bin/env python3
"""Render a rootless Podman pod spec, optionally launch it. Requires PyYAML."""
import argparse
import json
import os
import shlex
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import urlopen

import yaml


def podman_argv():
    return shlex.split(os.environ.get('PODMAN', 'podman'))


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
