#!/usr/bin/env python3
"""Exercise a built image in a disposable Podman pod; requires a capable host."""
import argparse
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import socket
import shutil
import subprocess
import tarfile
import tempfile
import time
from urllib.request import Request, urlopen
import uuid

from launch import podman_argv, render


def command(*args):
    return subprocess.check_output(podman_argv() + list(args), text=True)


def mapped_uid(status, uid_map):
    """Read the effective UID in the container's namespace, including rootless maps."""
    uids = next(line.split()[1:] for line in status.splitlines() if line.startswith('Uid:'))
    effective = int(uids[1])
    for line in uid_map.splitlines():
        inside, outside, length = map(int, line.split())
        if outside <= effective < outside + length:
            return inside + effective - outside
    raise AssertionError('effective process UID is outside the inspected namespace map')


def parse_mountinfo(text):
    def unescape(value):
        return re.sub(r'\\([0-7]{3})', lambda match: chr(int(match[1], 8)), value)
    mounts = []
    for line in text.splitlines():
        before, after = line.split(' - ', 1)
        fields, filesystem = before.split(), after.split()
        mounts.append({'id': int(fields[0]), 'parent_id': int(fields[1]),
                       'root': unescape(fields[3]), 'point': unescape(fields[4]),
                       'options': set(fields[5].split(',')), 'type': filesystem[0]})
    return mounts


def fdinfo_mount_id(text):
    entries = [line.split()[1:] for line in text.splitlines() if line.startswith('mnt_id:')]
    assert len(entries) == 1 and len(entries[0]) == 1 and entries[0][0].isdigit(), \
        'fdinfo does not identify exactly one kernel mount'
    return int(entries[0][0])


def path_mount_id(process, point):
    """Resolve the mount actually reached through the container's filesystem view."""
    fd = os.open(process / 'root' / point.lstrip('/'), os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        return fdinfo_mount_id(Path(f'/proc/self/fdinfo/{fd}').read_text())
    finally:
        os.close(fd)


def check_visible_mount(mounts, point, mount_id):
    # mountinfo can contain hidden mounts stacked at the same path. Neither
    # entry order nor a maximum ID establishes which mount path lookup reaches.
    entries = [mount for mount in mounts if mount['point'] == point]
    visible = [mount for mount in entries if mount['id'] == mount_id]
    assert len(visible) == 1, (f'visible mount at {point} unresolved: kernel ID {mount_id}, '
                               f'{len(entries)} entries, IDs {[mount["id"] for mount in entries]}')
    mount = visible[0]
    if point == '/tmp':
        assert mount['type'] == 'tmpfs' and 'rw' in mount['options'], \
            'visible /tmp mount must be writable tmpfs'
    else:
        assert 'ro' in mount['options'], f'{point} is not actually read-only'
    return mount


def cgroup_directory(membership, mounts):
    paths = [line[3:] for line in membership.splitlines() if line.startswith('0::')]
    assert len(paths) == 1, 'a Linux cgroup v2 runtime host is required'
    path = PurePosixPath(paths[0])
    assert path.is_absolute() and '..' not in path.parts, 'unsupported cgroup namespace path'
    for mount in mounts:
        if mount['type'] == 'cgroup2' and path.is_relative_to(mount['root']):
            return Path(mount['point']) / path.relative_to(mount['root'])
    raise AssertionError('container cgroup is not visible on this Linux runtime host')


def inspect_runtime_process(pid):
    """Verify actual kernel state without executing helpers inside the image."""
    assert isinstance(pid, int) and pid > 0, 'container has no live Linux host PID'
    process = Path('/proc') / str(pid)
    try:
        status = (process / 'status').read_text()
        uid_map = (process / 'uid_map').read_text()
        mounts = parse_mountinfo((process / 'mountinfo').read_text())
        membership = (process / 'cgroup').read_text()
        host_mounts = parse_mountinfo(Path('/proc/self/mountinfo').read_text())
        assert mapped_uid(status, uid_map) == 10001, 'effective container UID must be 10001'
        for point in ('/', '/packages', '/tmp'):
            check_visible_mount(mounts, point, path_mount_id(process, point))
        group = cgroup_directory(membership, host_mounts)
        memory = (group / 'memory.max').read_text().strip()
        cpu = (group / 'cpu.max').read_text().split()
        assert memory == '1073741824', 'actual cgroup memory limit must be 1 GiB'
        assert len(cpu) == 2 and cpu[0] != 'max'
        assert 0 < int(cpu[0]) / int(cpu[1]) <= 2, 'actual CPU quota must be at most two cores'
    except (PermissionError, FileNotFoundError) as error:
        raise RuntimeError('Smoke requires /proc (including the container root paths) and cgroup access on the local Linux Podman '
                           'runtime host; remote clients or insufficient rootless process '
                           'permissions cannot establish acceptance') from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True)
    args = parser.parse_args()
    name = 'flow-review-smoke-' + uuid.uuid4().hex[:10]
    with tempfile.TemporaryDirectory(prefix='flow-review-smoke-') as directory:
        root = Path(directory)
        root.chmod(0o755)
        package = root / 'packages' / 'check-total'
        fixture = Path(__file__).resolve().parents[3] / 'flows/starters/check-total'
        shutil.copytree(fixture, package)
        with socket.socket() as port_socket:
            port_socket.bind(('127.0.0.1', 0))
            port = port_socket.getsockname()[1]
        pod = render(root / 'packages', args.image, port, '127.0.0.1')
        pod['metadata']['name'] = name
        spec = root / 'pod.json'
        spec.write_text(json.dumps(pod))
        try:
            command('kube', 'play', str(spec))
            base = f'http://127.0.0.1:{port}'
            for _ in range(60):
                try:
                    with urlopen(base + '/healthz', timeout=1) as response:
                        assert json.load(response)['status'] == 'ok'
                    break
                except OSError:
                    time.sleep(0.25)
            else:
                raise AssertionError('Image never became healthy')
            with urlopen(base + '/', timeout=5) as response:
                assert b'<html' in response.read().lower()
            with urlopen(base + '/api/packages', timeout=5) as response:
                packages = json.load(response)['packages']
                assert len(packages) == 1
            with urlopen(base + '/api/packages/' + packages[0]['id'], timeout=5) as response:
                document = json.load(response)
                assert document['valid'] is True, document['diagnostics']
                assert document['setup'] is not None
                assert len(document['presentation']['phases']) == 3
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode='w') as tar:
                for path in sorted(package.rglob('*')):
                    if path.is_file():
                        data = path.read_bytes()
                        item = tarfile.TarInfo(path.relative_to(package).as_posix())
                        item.size = len(data)
                        tar.addfile(item, io.BytesIO(data))
            request = Request(base + '/api/upload', data=archive.getvalue(),
                              headers={'Content-Type': 'application/octet-stream'})
            with urlopen(request, timeout=10) as response:
                document = json.load(response)
                assert document['valid'] is True, document['diagnostics']
                assert document['setup'] is not None
                assert len(document['presentation']['phases']) == 3
            container = name + '-server'
            inspection = json.loads(command('inspect', container))[0]
            assert inspection['Config']['User'] == '10001:10001'
            assert inspection['HostConfig']['ReadonlyRootfs'] is True
            assert inspection['HostConfig']['Memory'] > 0
            limits = inspection['HostConfig']
            assert limits.get('NanoCpus', 0) > 0 or limits.get('CpuQuota', 0) > 0
            # Podman reports an early manual check as starting/unhealthy until
            # its configured start grace elapses, even when HTTP is ready.
            grace = inspection['Config']['Healthcheck'].get('StartPeriod', 0) / 1e9
            assert 0 <= grace <= 30, 'unsupported healthcheck start grace'
            time.sleep(grace + 0.25)
            command('healthcheck', 'run', container)
            assert any(m['Destination'] == '/packages' and not m['RW'] for m in inspection['Mounts'])
            inspect_runtime_process(inspection['State']['Pid'])
            snapshot = root / 'transient-snapshot'
            snapshot.mkdir()
            command('cp', container + ':/tmp/trollm-flow-review/.', str(snapshot))
            assert not any(snapshot.iterdir()), 'inspection staging was not removed'
            print('Podman image smoke passed: assets, health, mount/import, cleanup, non-root, read-only, tmpfs, memory')
        finally:
            subprocess.run(podman_argv() + ['pod', 'rm', '-f', name], check=False)


if __name__ == '__main__':
    main()
