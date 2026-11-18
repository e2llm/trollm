#!/usr/bin/env python3
"""Collect exact dependency notices from resolved build inputs, without weights."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

RUST_STDLIB_LICENSES = {'Apache-2.0.txt', 'MIT.txt', 'LLVM-exception.txt'}


def runtime_packages(metadata):
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    root = next(p['id'] for p in packages.values() if p['name'] == 'trollm-uniffi')
    pending, seen = [root], set()
    while pending:
        identifier = pending.pop()
        if identifier in seen:
            continue
        seen.add(identifier)
        pending.extend(dep['pkg'] for dep in nodes[identifier]['deps']
                       if any(kind['kind'] is None for kind in dep['dep_kinds']))
    return sorted((packages[p] for p in seen if packages[p]['source'] is not None),
                  key=lambda p: (p['name'], p['version']))


def combine_metadata(documents):
    """Retain the union of runtime dependency edges for every packaged ABI."""
    packages, nodes = {}, {}
    for document in documents:
        packages.update((p['id'], p) for p in document['packages'])
        for node in document['resolve']['nodes']:
            edges = nodes.setdefault(node['id'], {})
            for dependency in node['deps']:
                edges[json.dumps(dependency, sort_keys=True)] = dependency
    return {'packages': list(packages.values()), 'resolve': {'nodes': [
        {'id': identifier, 'deps': list(edges.values())} for identifier, edges in nodes.items()]}}


def license_files(directory):
    prefixes = ('LICENSE', 'LICENCE', 'NOTICE', 'COPYING', 'COPYRIGHT')
    return sorted(p for p in directory.rglob('*') if p.is_file() and not p.is_symlink()
                  and p.name.upper().startswith(prefixes))


def rust_toolchain_notices(sysroot):
    """Read the installed standard-library copyright and accompanying license texts."""
    stdlib = sysroot / 'share/doc/rust'
    copyright_file = stdlib / 'COPYRIGHT-library.html'
    terms = sorted(p for p in (stdlib / 'licenses').glob('*.txt') if p.is_file())
    if not copyright_file.is_file() or not RUST_STDLIB_LICENSES <= {p.name for p in terms}:
        raise ValueError('Rust toolchain standard-library copyright or license texts unavailable')
    for path in [copyright_file, *terms]:
        source = path.relative_to(sysroot).as_posix()
        data = path.read_bytes()
        if not data.strip():
            raise ValueError('empty Rust toolchain notice: ' + source)
        yield source, data


def collect(root, metadata, android, ndk, output, rust_sysroot):
    entries, sections = [], []

    def add(label, data, source):
        if len(data) > 16 * 1024 * 1024:
            raise ValueError('notice file exceeds bound: ' + label)
        text = data.decode('utf-8')
        entries.append({'name': label, 'source': source, 'bytes': len(data),
                        'sha256': hashlib.sha256(data).hexdigest()})
        sections.append('\n' + '=' * 72 + '\n' + label + '\n' + source + '\n\n' + text)

    add('trollm LICENSE', (root / 'LICENSE').read_bytes(), 'trollm source distribution')
    add('trollm dependency guide', (root / 'THIRD_PARTY_NOTICES.md').read_bytes(),
        'THIRD_PARTY_NOTICES.md in the matching source distribution')
    supplemental = root / 'licenses' / 'upstream'
    for record in json.loads((supplemental / 'sources.json').read_text()):
        data = (supplemental / record['file']).read_bytes()
        if hashlib.sha256(data).hexdigest() != record['sha256']:
            raise ValueError('pinned supplemental notice changed')
        add('Supplemental ' + record['file'], data, record['source'])
    packages = runtime_packages(metadata)
    for package in packages:
        directory = Path(package['manifest_path']).parent
        files = license_files(directory)
        declared = package.get('license') or package.get('license_file')
        if not declared:
            raise ValueError('missing Rust license declaration: ' + package['name'])
        fallback = (package['name'].startswith('uniffi') or
                    package['name'] in ('llama-cpp-2', 'llama-cpp-sys-2',
                                        'uuid-simd', 'vsimd', 'jsonschema-regex', 'jsonschema-value'))
        if not files and not fallback:
            raise ValueError('missing Rust license text: ' + package['name'])
        label = package['name'] + ' ' + package['version']
        source = 'https://docs.rs/crate/' + package['name'] + '/' + package['version'] + '/source/'
        sections.append('\nRust package: ' + label + '\nDeclared license: ' + declared + '\n' + source + '\n')
        for path in files:
            relative = path.relative_to(directory).as_posix()
            add(label + ' / ' + relative, path.read_bytes(), source + relative)
        if fallback:
            sections.append('See the pinned supplemental upstream workspace license above.\n')
    for source, data in rust_toolchain_notices(rust_sysroot):
        add('Rust toolchain / ' + source, data, 'Installed Rust toolchain: ' + source)
    for path in license_files(root / 'crates/trollm-mcp/vendor/rpa-kernel-clock'):
        add('Moray kernel clock / ' + path.name, path.read_bytes(), 'Apache-2.0 grant; matching source directory')
    if not android.get('components'):
        raise ValueError('empty Android notice inventory')
    for component in android['components']:
        if not component['licenses']:
            raise ValueError('missing Maven POM license: ' + component['coordinate'])
        sections.append('\nAndroid component: ' + component['coordinate'] + '\n')
        sections.append('License declaration from POM: ' + component.get('licenseSource', component['coordinate']) + '\n')
        for license in component['licenses']:
            sections.append('Declared license: ' + license['name'] + '\n' + license['url'] + '\n')
        for notice in component['notices']:
            add(component['coordinate'] + ' / ' + notice['name'], notice['text'].encode('utf-8'),
                'Embedded notice from the resolved Maven artifact')
        # Apache-2.0's complete text is included above. JNA's exact pinned dual
        # license and its libffi notice are also retained in supplemental texts.
        apache = any('apache' in (x['name'] + x['url']).lower() for x in component['licenses'])
        if not apache and not component['notices'] and not component['coordinate'].startswith('net.java.dev.jna:'):
            raise ValueError('non-Apache Maven component needs exact license text: ' + component['coordinate'])
    ndk_files = sorted(set(ndk.glob('NOTICE*')) | set(ndk.glob('toolchains/llvm/prebuilt/*/NOTICE*')))
    ndk_files = [p for p in ndk_files if p.is_file()]
    if not ndk_files:
        raise ValueError('NDK notices unavailable')
    for path in ndk_files:
        add('Android NDK / ' + path.relative_to(ndk).as_posix(), path.read_bytes(),
            'Installed Android NDK 27.2.12479018 build input, including toolchain runtime notices')
    version = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    header = (
        f'trollm {version} — third-party notices\n\n'
        'Collected from the resolved Rust runtime dependency graph, the installed Rust\n'
        'toolchain standard-library notice bundle, resolved Android runtime POMs and\n'
        'embedded artifact notices, vendored native source, and the installed NDK.\n'
        'Native inventories include notices for bundled optional\n'
        'backends; listing a component does not imply every feature was linked.\n'
        'Model weights are separate downloads and are not distributed in this APK.\n'
        'UniFFI covered source is available at the exact upstream revision recorded\n'
        'in the supplemental source links and the matching trollm source release.\n'
    )
    text = header + ''.join(sections)
    if len(text.encode('utf-8')) > 48 * 1024 * 1024:
        raise ValueError('combined notice file exceeds bound')
    output.mkdir(parents=True, exist_ok=True)
    (output / 'THIRD_PARTY_NOTICES.txt').write_text(text)
    manifest = {'format': 'trollm.apk-notices.v1', 'rustPackages': len(packages),
                'androidComponents': len(android['components']), 'files': entries,
                'noticesSha256': hashlib.sha256(text.encode('utf-8')).hexdigest()}
    (output / 'third-party-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('APK notices:', len(packages), 'Rust packages,', len(android['components']), 'Maven components')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--android-inventory', type=Path, required=True)
    parser.add_argument('--ndk', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--backend', choices=('stub', 'llama'), required=True)
    parser.add_argument('--cargo-metadata', type=Path)
    parser.add_argument('--rust-sysroot', type=Path, help='override the selected rustc sysroot')
    parser.add_argument('--delivery-output', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if args.cargo_metadata:
        metadata = json.loads(args.cargo_metadata.read_text())
    else:
        metadata = combine_metadata([json.loads(subprocess.check_output([
            'cargo', 'metadata', '--locked', '--format-version', '1',
            '--filter-platform', target, '--features', 'trollm-uniffi/' + args.backend,
        ], cwd=root)) for target in (
            'aarch64-linux-android', 'armv7-linux-androideabi', 'x86_64-linux-android')])
    rust_sysroot = args.rust_sysroot or Path(subprocess.check_output(
        [os.environ.get('RUSTC') or 'rustc', '--print', 'sysroot'], cwd=root, text=True).strip())
    collect(root, metadata, json.loads(args.android_inventory.read_text()), args.ndk, args.output, rust_sysroot)
    if args.delivery_output:
        args.delivery_output.mkdir(parents=True, exist_ok=True)
        for name in ('THIRD_PARTY_NOTICES.txt', 'third-party-manifest.json'):
            shutil.copyfile(args.output / name, args.delivery_output / name)


if __name__ == '__main__':
    main()
