#!/usr/bin/env python3
"""Read the shared Rust/Android release identity without a compiler."""
import argparse
import json
from pathlib import Path
import re
import tomllib

REPO = Path(__file__).resolve().parent.parent
# SemVer 2.0: numeric core and prerelease identifiers cannot have leading zeros.
SEMVER = re.compile(
    r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
    r'(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
    r'(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?'
    r'(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?'
)


def read_toml(path):
    with path.open('rb') as stream:
        return tomllib.load(stream)


def read_version(repo: Path = REPO) -> dict:
    workspace = read_toml(repo / 'Cargo.toml').get('workspace', {})
    version = workspace.get('package', {}).get('version')
    code = workspace.get('metadata', {}).get('android', {}).get('version-code')
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        raise ValueError('workspace.package.version must be a valid SemVer string')
    if type(code) is not int or not 1 <= code <= 2_100_000_000:
        raise ValueError('workspace.metadata.android.version-code must be an integer in 1..2100000000')
    return {'versionName': version, 'versionCode': code}


def check_consistency(repo: Path = REPO) -> dict:
    identity = read_version(repo)
    workspace = read_toml(repo / 'Cargo.toml')['workspace']
    locked = read_toml(repo / 'Cargo.lock').get('package', [])
    members = workspace.get('members', [])
    if not members:
        raise ValueError('workspace.members must declare the versioned crates')
    for member in members:
        paths = list(repo.glob(member))
        if not paths:
            raise ValueError(f'Workspace member does not exist: {member}')
        for path in paths:
            package = read_toml(path / 'Cargo.toml')['package']
            inherited = package.get('version')
            if (not isinstance(inherited, dict) or set(inherited) != {'workspace'}
                    or inherited['workspace'] is not True):
                raise ValueError(f'{member} must inherit version.workspace = true')
            matches = [entry for entry in locked
                       if entry.get('name') == package['name'] and 'source' not in entry]
            if len(matches) != 1 or matches[0].get('version') != identity['versionName']:
                raise ValueError(f'Cargo.lock version for {package["name"]} does not match workspace')
    return identity


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='also check workspace and lockfile versions')
    parser.add_argument('--repo', type=Path, default=REPO)
    args = parser.parse_args()
    try:
        identity = check_consistency(args.repo) if args.check else read_version(args.repo)
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        parser.exit(1, f'Invalid release version: {error}\n')
    print(json.dumps(identity))


if __name__ == '__main__':
    main()
