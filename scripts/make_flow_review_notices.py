#!/usr/bin/env python3
"""Collect or verify notices for locked Linux/amd64 server builds."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import stat
import struct
import tomllib

from android.make_apk_notices import RUST_STDLIB_LICENSES, license_files, rust_toolchain_notices

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'
SERVER = 'trollm-flow-review'
CRATES_IO = 'registry+https://github.com/rust-lang/crates.io-index'
FORMAT = 'trollm.flow-review-rust-notices.v1'
DESKTOP_FORMAT = 'trollm.desktop-rust-notices.v1'
DESKTOP_SERVER = 'trollm-desktop'
PLATFORM = 'linux/amd64'
TEXT_NAME = 'THIRD_PARTY_NOTICES.txt'
MANIFEST_NAME = 'third-party-manifest.json'
LICENSE_PREFIXES = ('LICENSE', 'LICENCE', 'COPYING')
SUPPLEMENTS = {
    ('uniffi', '0.28.3'): 'uniffi-LICENSE.txt',
    ('uniffi_checksum_derive', '0.28.3'): 'uniffi-LICENSE.txt',
    ('uniffi_core', '0.28.3'): 'uniffi-LICENSE.txt',
    ('uniffi_macros', '0.28.3'): 'uniffi-LICENSE.txt',
    ('uniffi_meta', '0.28.3'): 'uniffi-LICENSE.txt',
    ('jsonschema-regex', '0.58.6'): 'jsonschema-LICENSE.txt',
    ('jsonschema-value', '0.58.6'): 'jsonschema-LICENSE.txt',
    ('uuid-simd', '0.8.0'): 'simd-LICENSE.txt',
    ('vsimd', '0.8.0'): 'simd-LICENSE.txt',
}

LLAMA_PACKAGES = {'llama-cpp-2', 'llama-cpp-sys-2'}
LLAMA_WRAPPER_LICENSES = ('llama-cpp-rs-LICENSE-APACHE.txt', 'llama-cpp-rs-LICENSE-MIT.txt')
LLAMA_NATIVE_LICENSE = 'llama.cpp/LICENSE'
CUDA_HELPER_LICENSES = ('rust-cuda-LICENSE-APACHE.txt', 'rust-cuda-LICENSE-MIT.txt')
DESKTOP_SUPPLEMENTS = {
    ('llama-cpp-2', '0.1.159'): LLAMA_WRAPPER_LICENSES,
    ('llama-cpp-sys-2', '0.1.159'): LLAMA_WRAPPER_LICENSES,
    ('find_cuda_helper', '0.2.0'): CUDA_HELPER_LICENSES,
}
DESKTOP_LLAMA_WORKSPACE = {'crates/trollm-uniffi/Cargo.toml', 'crates/trollm-models/Cargo.toml'}
COMPONENTS = {
    'flow-review': (SERVER, FORMAT, 'flow-review server'),
    'desktop': (DESKTOP_SERVER, DESKTOP_FORMAT, 'desktop host'),
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def compiled_packages(root, messages, lock, component='flow-review'):
    """Read artifact records from Cargo's release build, including fresh artifacts."""
    locked = {(p['name'], p['version']): p for p in lock['package'] if p.get('source') == CRATES_IO}
    server, _, _ = COMPONENTS[component]
    workspace = root / f'apps/{component}/server/Cargo.toml'
    packages = {}
    llama_workspace = set()
    saw_server = saw_success = False
    for line in messages:
        # Cargo allows non-JSON output from procedural macros.
        if not line.startswith('{'):
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        reason = message.get('reason')
        if reason == 'build-finished':
            saw_success = message.get('success') is True
        if reason != 'compiler-artifact':
            continue
        manifest = Path(message['manifest_path']).resolve(strict=True)
        package_id = message['package_id']
        if package_id.startswith('path+file:') and manifest.is_relative_to(root):
            executable = message.get('executable')
            if manifest == workspace and message['target']['name'] == server and executable:
                saw_server = Path(executable).resolve() == release_binary(root, server)
                if component == 'desktop' and saw_server:
                    require_llama_features(message['features'])
                    profile = message.get('profile', {})
                    if (message['target']['kind'] != ['bin'] or profile.get('test') is not False
                            or profile.get('debug_assertions') is not False
                            or profile.get('opt_level') not in {'1', '2', '3', 's', 'z'}):
                        raise ValueError('expected the desktop release binary artifact')
            if component == 'desktop':
                relative = manifest.relative_to(root).as_posix()
                if relative in DESKTOP_LLAMA_WORKSPACE:
                    require_llama_features(message['features'])
                    llama_workspace.add(relative)
            continue
        if not package_id.startswith(CRATES_IO + '#'):
            raise ValueError('unsupported source for compiled Rust package')
        directory = manifest.parent
        package = tomllib.loads(manifest.read_text())['package']
        key = package['name'], package['version']
        if package_id != CRATES_IO + '#' + '@'.join(key) or key not in locked:
            raise ValueError('compiled package does not match Cargo.lock: ' + '@'.join(key))
        checksum = locked[key].get('checksum', '')
        if not re.fullmatch(r'[0-9a-f]{64}', checksum):
            raise ValueError('missing crate archive checksum: ' + '@'.join(key))
        entry = packages.setdefault(key, (directory, package, checksum, set(), set()))
        if directory != entry[0]:
            raise ValueError('multiple source directories for compiled package: ' + '@'.join(key))
        entry[3].update(message['features'])
        entry[4].update(message['target']['kind'])
    if not saw_success or not saw_server or not packages:
        raise ValueError(f'expected successful {component} release build with Rust dependencies for {TARGET}')
    if component == 'desktop':
        if llama_workspace != DESKTOP_LLAMA_WORKSPACE:
            raise ValueError('required workspace llama artifacts absent')
        llama_libraries = {name for (name, _), entry in packages.items()
                           if 'lib' in entry[4] and name in LLAMA_PACKAGES}
        if llama_libraries != LLAMA_PACKAGES:
            raise ValueError('required compiled llama library artifacts absent')
    return packages


def require_llama_features(features):
    if 'llama' not in features or 'stub' in features:
        raise ValueError('desktop release requires llama and excludes stub')


def release_binary(root, server):
    return root / 'target' / TARGET / 'release' / server


def desktop_binary_sha256(root):
    path = release_binary(root, DESKTOP_SERVER)
    if path.is_symlink() or not path.is_file() or not path.stat().st_mode & stat.S_IXUSR:
        raise ValueError('desktop release artifact is missing or not executable')
    with path.open('rb') as binary:
        header = binary.read(64)
        # Linux x86-64 executables may be PIE (ET_DYN) or ET_EXEC. Require an
        # entry point and a program-header table as well as the ELF identity.
        if (len(header) < 64 or not header.startswith(b'\x7fELF\x02\x01\x01')
                or header[7] not in (0, 3)):
            raise ValueError('desktop release artifact is not a Linux/amd64 ELF executable')
        kind, machine, version, entry, program_headers = struct.unpack_from('<HHIQQ', header, 16)
        size = path.stat().st_size
        program_size, program_count = struct.unpack_from('<HH', header, 54)
        if (kind not in (2, 3) or machine != 62 or version != 1 or entry == 0
                or program_headers < 64 or program_size != 56 or program_count == 0
                or program_headers + program_size * program_count > size):
            raise ValueError('desktop release artifact is not a Linux/amd64 ELF executable')
        binary.seek(program_headers)
        for _ in range(program_count):
            segment, flags, offset, address, _, length, _, _ = struct.unpack('<IIQQQQQQ', binary.read(56))
            if segment == 1 and flags & 1 and address <= entry < address + length and offset + length <= size:
                break
        else:
            raise ValueError('desktop release artifact has no executable ELF entry segment')
        binary.seek(0)
        return hashlib.file_digest(binary, 'sha256').hexdigest()


def collect(root, messages, sysroot, output, component='flow-review'):
    lock_bytes = (root / 'Cargo.lock').read_bytes()
    _, notice_format, label = COMPONENTS[component]
    compiled = compiled_packages(root, messages, tomllib.loads(lock_bytes.decode('utf-8')), component)
    desktop = component == 'desktop'
    binary_sha = desktop_binary_sha256(root) if desktop else None
    upstream = root / 'licenses/upstream'
    supplemental = {r['file']: r for r in json.loads((upstream / 'sources.json').read_text())}
    platform = f'{PLATFORM}, {TARGET}' if desktop else TARGET
    lines = [
        f'trollm {label} ({platform}) — third-party notices\n\n',
        'Cargo reported these packages while building the locked release server.\n',
        'The inventory includes build dependencies and procedural macros, as well as\n',
        'notices for optional code packaged in the source of a compiled dependency.\n',
        'UniFFI covered source is available from the exact upstream revision recorded\n',
        'in its supplemental source link and the matching trollm source release.\n',
    ]
    body = bytearray(''.join(lines).encode('utf-8'))
    files = []

    def add(label, source, data):
        if not data.strip() or len(data) > 16 * 1024 * 1024:
            raise ValueError('empty or oversized notice file: ' + label)
        data.decode('utf-8')
        body.extend(f'\n{"=" * 72}\n{label}\n{source}\n\n'.encode('utf-8'))
        files.append({'name': label, 'source': source, 'offset': len(body), 'bytes': len(data),
                      'sha256': sha256(data)})
        body.extend(data)
        return label

    packages = []
    for (name, version), (directory, manifest, checksum, features, kinds) in sorted(compiled.items()):
        label = f'{name} {version}'
        published_source = f'https://docs.rs/crate/{name}/{version}/source/'
        declared_file = manifest.get('license-file')
        declared = manifest.get('license') or declared_file
        if not declared:
            raise ValueError('missing Rust license declaration: ' + label)
        texts = set(license_files(directory))
        if declared_file:
            path = directory / declared_file
            if (Path(declared_file).is_absolute() or not path.resolve().is_relative_to(directory)
                    or not path.is_file() or path.is_symlink()):
                raise ValueError('missing or invalid declared license file: ' + label)
            texts.add(path.resolve())
        pinned = SUPPLEMENTS.get((name, version))
        pins = (pinned,) if pinned else ()
        if desktop:
            pins = DESKTOP_SUPPLEMENTS.get((name, version), pins)
        if not (declared_file or pins or any(p.parent == directory and
                p.name.upper().startswith(LICENSE_PREFIXES) for p in texts)):
            raise ValueError('missing Rust license text: ' + label)
        body.extend(f'\nRust package: {label}\nDeclared license: {declared}\n'.encode('utf-8'))
        notice_labels = [add(f'{label} / {p.relative_to(directory).as_posix()}',
                             published_source + p.relative_to(directory).as_posix(), p.read_bytes())
                         for p in sorted(texts)]
        for pin in pins:
            record = supplemental[pin]
            data = (upstream / pin).read_bytes()
            if sha256(data) != record['sha256']:
                raise ValueError('pinned supplemental notice changed: ' + pin)
            notice_labels.append(add(f'{label} / Supplemental {pin}', record['source'], data))
        packages.append({'name': name, 'version': version, 'crateSha256': checksum,
                         'declaredLicense': declared, 'features': sorted(features),
                         'targetKinds': sorted(kinds), 'notices': notice_labels})

    vendor = 'crates/trollm-mcp/vendor/rpa-kernel-clock/'
    for name in ('LICENSE', 'NOTICE'):
        source = vendor + name
        add('Moray kernel clock / ' + name, source, (root / source).read_bytes())

    for source, data in rust_toolchain_notices(sysroot):
        add('Rust toolchain / ' + source, 'Installed Rust toolchain: ' + source, data)

    if len(body) > 48 * 1024 * 1024:
        raise ValueError('combined Rust notice text exceeds bound')
    manifest = {'format': notice_format, 'target': TARGET, 'profile': 'release',
                'cargoLockSha256': sha256(lock_bytes), 'packages': packages,
                'files': files, 'noticesSha256': sha256(body)}
    if desktop:
        manifest.update(platform=PLATFORM, backend='llama', binarySha256=binary_sha)
    verify(body, manifest, component)
    output.mkdir(parents=True, exist_ok=True)
    (output / TEXT_NAME).write_bytes(body)
    (output / MANIFEST_NAME).write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Collected {len(packages)} compiled Rust packages and {len(files)} notice files')


def verify(body, manifest, component='flow-review'):
    _, notice_format, _ = COMPONENTS[component]
    if (manifest['format'], manifest['target'], manifest['profile']) != (notice_format, TARGET, 'release'):
        raise ValueError('unexpected Rust notice identity')
    desktop = component == 'desktop'
    if desktop and (manifest.get('platform'), manifest.get('backend')) != (PLATFORM, 'llama'):
        raise ValueError('unexpected desktop platform or backend')
    if desktop and not re.fullmatch(r'[0-9a-f]{64}', manifest.get('binarySha256', '')):
        raise ValueError('missing desktop release artifact checksum')
    if manifest['noticesSha256'] != sha256(body):
        raise ValueError('Rust notice text differs from its manifest')
    end = 0
    files = set()
    for file in manifest['files']:
        start = file['offset']
        data = body[start:start + file['bytes']]
        if (start < end or not data or len(data) != file['bytes'] or sha256(data) != file['sha256']
                or file['name'] in files):
            raise ValueError('invalid Rust notice text entry: ' + file['name'])
        files.add(file['name'])
        end = start + file['bytes']
    crate_files = {name for name in files if not name.startswith(('Moray kernel clock / ', 'Rust toolchain / '))}
    reported_files = set()
    for package in manifest['packages']:
        prefix = f'{package["name"]} {package["version"]} / '
        if not package['notices'] or any(n not in crate_files or not n.startswith(prefix)
                                          for n in package['notices']):
            raise ValueError('missing Rust package notice: ' + package['name'])
        reported_files.update(package['notices'])
        if desktop and package['name'] in LLAMA_PACKAGES:
            expected = {prefix + 'Supplemental ' + pin for pin in LLAMA_WRAPPER_LICENSES}
            if package['name'] == 'llama-cpp-sys-2':
                expected.add(prefix + LLAMA_NATIVE_LICENSE)
            if 'lib' not in package['targetKinds'] or not expected <= set(package['notices']):
                raise ValueError('required compiled llama license text absent: ' + package['name'])
    required = {'axum', 'tokio', 'tower-http', 'ring', 'uniffi'}
    if desktop:
        required |= LLAMA_PACKAGES
    if not required <= {p['name'] for p in manifest['packages']}:
        raise ValueError('required server dependencies absent from the Rust notices')
    if reported_files != crate_files:
        raise ValueError('Rust notice text entries differ from the package inventory')
    if not ({'Moray kernel clock / LICENSE', 'Moray kernel clock / NOTICE',
             'Rust toolchain / share/doc/rust/COPYRIGHT-library.html'} |
            {'Rust toolchain / share/doc/rust/licenses/' + name for name in RUST_STDLIB_LICENSES}) <= files:
        raise ValueError('required embedded-code or Rust toolchain notices absent')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest='command', required=True)
    build = subcommands.add_parser('collect')
    build.add_argument('--build-messages', type=Path, required=True)
    build.add_argument('--sysroot', type=Path, required=True)
    build.add_argument('directory', type=Path)
    check = subcommands.add_parser('verify')
    check.add_argument('directory', type=Path)
    for command in (build, check):
        command.add_argument('--component', choices=COMPONENTS, default='flow-review')
    args = parser.parse_args()
    if args.command == 'collect':
        with args.build_messages.open() as messages:
            collect(ROOT, messages, args.sysroot, args.directory, args.component)
    else:
        manifest = json.loads((args.directory / MANIFEST_NAME).read_text())
        if manifest['cargoLockSha256'] != sha256((ROOT / 'Cargo.lock').read_bytes()):
            raise ValueError('Rust notice manifest does not match the source Cargo.lock')
        verify((args.directory / TEXT_NAME).read_bytes(), manifest, args.component)
        if args.component == 'desktop' and manifest['binarySha256'] != desktop_binary_sha256(ROOT):
            raise ValueError('desktop release artifact differs from the Rust notice manifest')
        print(f'Verified {args.component} Rust notice text and manifest')


if __name__ == '__main__':
    main()
