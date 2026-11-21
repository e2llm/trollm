import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from make_flow_review_notices import (CRATES_IO, CUDA_HELPER_LICENSES, DESKTOP_FORMAT, DESKTOP_SERVER,
                                     LLAMA_NATIVE_LICENSE, LLAMA_PACKAGES,
                                     LLAMA_WRAPPER_LICENSES, MANIFEST_NAME,
                                     SERVER, TARGET, TEXT_NAME, collect, main,
                                     sha256, verify)


class NoticeBuildFixture(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.base = Path(folder.name)
        self.root = self.base / 'checkout'
        self.sysroot = self.base / 'rust toolchain'
        self.output = self.base / 'output'
        workspace = self.write('checkout/apps/flow-review/server/Cargo.toml', b'')
        self.write('checkout/crates/trollm-mcp/vendor/rpa-kernel-clock/LICENSE', b'Vendor terms\n')
        self.write('checkout/crates/trollm-mcp/vendor/rpa-kernel-clock/NOTICE', b'Vendor attribution\n')
        self.write('rust toolchain/share/doc/rust/COPYRIGHT-library.html', b'<p>Copyright</p>\n')
        for name in ('Apache-2.0', 'MIT', 'LLVM-exception'):
            self.write(f'rust toolchain/share/doc/rust/licenses/{name}.txt', b'Toolchain terms\n')
        self.upstream = b'Pinned upstream terms\n'
        self.write('checkout/licenses/upstream/uniffi-LICENSE.txt', self.upstream)
        self.write('checkout/licenses/upstream/sources.json', json.dumps([{
            'file': 'uniffi-LICENSE.txt', 'source': 'https://example.org/fixture/LICENSE',
            'sha256': sha256(self.upstream),
        }]).encode())
        self.records = [self.artifact(workspace, f'path+file://{workspace.parent}#1.0.0', SERVER,
                                      ['bin'], [], str(self.root / 'target' / TARGET / 'release' / SERVER))]
        lock = ''
        for name in ('axum', 'tokio', 'tower-http', 'ring', 'uniffi', 'unused', 'build-helper'):
            version = '0.28.3' if name == 'uniffi' else '1.0.0'
            lock += (f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
                     f'source = "{CRATES_IO}"\nchecksum = "{"f" * 64}"\n')
            declaration = 'license-file = "terms/LICENSE.txt"' if name == 'tokio' else 'license = "MIT"'
            manifest = self.write(f'cargo cache/{name}/Cargo.toml',
                                  f'[package]\nname = "{name}"\nversion = "{version}"\n{declaration}\n'.encode())
            if name != 'uniffi':
                relative = 'terms/LICENSE.txt' if name == 'tokio' else 'LICENSE'
                self.write(f'cargo cache/{name}/{relative}', f'{name} terms\r\n'.encode())
            if name != 'unused':
                package_id = f'{CRATES_IO}#{name}@{version}'
                self.records.append(self.artifact(manifest, package_id, name,
                                     ['proc-macro'] if name == 'build-helper' else ['lib'], ['first']))
                if name == 'ring':
                    self.records.append(self.artifact(manifest, package_id, name,
                                         ['custom-build'], ['second']))
        self.write('checkout/Cargo.lock', lock.encode())
        self.write('cargo cache/ring/vendor/COPYRIGHT', b'Exact embedded legal text.\n')
        (self.base / 'cargo cache/ring/vendor/LICENSE-symlink').symlink_to(self.root / 'Cargo.lock')
        self.records.append({'reason': 'build-finished', 'success': True})

    def write(self, relative, data):
        path = self.base / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def artifact(self, manifest, package_id, name, kinds, features, executable=None):
        return {'reason': 'compiler-artifact', 'manifest_path': str(manifest),
                'package_id': package_id, 'target': {'kind': kinds, 'name': name},
                'features': features, 'executable': executable}

    def generate(self, **options):
        collect(self.root, map(json.dumps, self.records), self.sysroot, self.output, **options)
        return ((self.output / TEXT_NAME).read_bytes(),
                json.loads((self.output / MANIFEST_NAME).read_text()))


class FlowReviewNoticesTest(NoticeBuildFixture):
    def test_compiled_inputs_embedded_notices_and_package_identities(self):
        text, manifest = self.generate()
        packages = {package['name']: package for package in manifest['packages']}
        self.assertNotIn('unused', packages)
        self.assertIn('build-helper', packages)
        self.assertEqual(packages['ring']['features'], ['first', 'second'])
        self.assertEqual(packages['ring']['targetKinds'], ['custom-build', 'lib'])
        self.assertIn(b'ring terms\r\n', text)
        self.assertIn(b'Exact embedded legal text.\n', text)
        self.assertIn(b'<p>Copyright</p>\n', text)
        self.assertIn(self.upstream, text)
        self.assertNotIn(b'LICENSE-symlink', text)
        self.assertNotIn(str(self.base).encode(), text)
        self.assertNotIn(str(self.base), json.dumps(manifest))
        self.assertEqual(manifest['cargoLockSha256'], sha256((self.root / 'Cargo.lock').read_bytes()))
        self.assertEqual(self.generate(), (text, manifest))

    def test_missing_license_text_or_changed_exact_pin_fails(self):
        self.write('checkout/licenses/upstream/uniffi-LICENSE.txt', b'Changed terms\n')
        with self.assertRaisesRegex(ValueError, 'pinned supplemental notice changed'):
            self.generate()
        self.write('checkout/licenses/upstream/uniffi-LICENSE.txt', self.upstream)
        (self.base / 'cargo cache/ring/LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, 'missing Rust license text: ring'):
            self.generate()

    def test_non_cargo_stdout_can_precede_artifact_records(self):
        lines = ['procedural macro output', '{custom output', '{"macro": "value"}']
        lines.extend(map(json.dumps, self.records))
        collect(self.root, lines, self.sysroot, self.output)
        self.assertTrue((self.output / MANIFEST_NAME).is_file())

    def test_failed_or_wrong_platform_build_is_rejected(self):
        self.records[-1]['success'] = False
        with self.assertRaisesRegex(ValueError, 'expected successful flow-review release build'):
            self.generate()
        self.records[-1]['success'] = True
        self.records[0]['executable'] = self.records[0]['executable'].replace(TARGET, 'aarch64-unknown-linux-gnu')
        with self.assertRaisesRegex(ValueError, 'expected successful flow-review release build'):
            self.generate()

    def test_extracted_notice_manifest_detects_hash_and_inventory_changes(self):
        text, manifest = self.generate()
        with self.assertRaisesRegex(ValueError, 'text differs'):
            verify(text + b'changed', manifest)
        manifest['files'][0]['sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'invalid Rust notice text entry'):
            verify(text, manifest)
        _, manifest = self.generate()
        manifest['packages'].pop(1)
        with self.assertRaisesRegex(ValueError, 'differ from the package inventory'):
            verify(text, manifest)


class DesktopNoticesTest(NoticeBuildFixture):
    def setUp(self):
        super().setUp()
        workspace = self.write('checkout/apps/desktop/server/Cargo.toml', b'')
        elf = self.linux_executable_fixture()
        self.binary = self.write(f'checkout/target/{TARGET}/release/{DESKTOP_SERVER}', elf)
        self.binary.chmod(0o755)
        self.records[0] = self.artifact(workspace, f'path+file://{workspace.parent}#1.0.0',
                                        DESKTOP_SERVER, ['bin'], ['default', 'llama'], str(self.binary))
        self.records[0]['profile'] = {'opt_level': '3', 'debug_assertions': False, 'test': False}
        self.build_finished = self.records.pop()
        for workspace_name in ('trollm-uniffi', 'trollm-models'):
            workspace = self.write(f'checkout/crates/{workspace_name}/Cargo.toml', b'')
            self.records.append(self.artifact(workspace, f'path+file://{workspace.parent}#1.0.0',
                                              workspace_name, ['lib'], ['llama']))
        self.write(f'cargo cache/llama-cpp-sys-2/{LLAMA_NATIVE_LICENSE}', b'Named native license\n')
        self.write('cargo cache/llama-cpp-sys-2/llama.cpp/vendor/distinct/LICENSE', b'Embedded notice\n')
        with (self.root / 'Cargo.lock').open('a') as lock:
            for name in sorted(LLAMA_PACKAGES | {'find_cuda_helper'}):
                version = '0.2.0' if name == 'find_cuda_helper' else '0.1.159'
                lock.write(f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
                           f'source = "{CRATES_IO}"\nchecksum = "{"b" * 64}"\n')
                manifest = self.write(f'cargo cache/{name}/Cargo.toml',
                                      f'[package]\nname = "{name}"\nversion = "{version}"\n'
                                      'license = "MIT OR Apache-2.0"\n'.encode())
                self.records.append(self.artifact(manifest, f'{CRATES_IO}#{name}@{version}',
                                                  name, ['lib'], []))
        self.records.append(self.build_finished)
        sources = self.root / 'licenses/upstream/sources.json'
        records = json.loads(sources.read_text())
        for name in LLAMA_WRAPPER_LICENSES + CUDA_HELPER_LICENSES:
            data = f'Pinned {name}\n'.encode()
            self.write('checkout/licenses/upstream/' + name, data)
            records.append({'file': name, 'source': 'https://example.org/fixture/' + name,
                            'sha256': sha256(data)})
        sources.write_text(json.dumps(records))

    @staticmethod
    def linux_executable_fixture():
        code = b'\xb8\x3c\x00\x00\x00\x31\xff\x0f\x05'
        address = 0x400000
        size = 64 + 56 + len(code)
        identity = b'\x7fELF\x02\x01\x01' + bytes(9)
        header = struct.pack('<HHIQQQIHHHHHH', 2, 62, 1, address + 120, 64, 0, 0, 64, 56, 1, 0, 0, 0)
        segment = struct.pack('<IIQQQQQQ', 1, 5, 0, address, address, size, size, 4096)
        return identity + header + segment + code

    def generate_desktop(self):
        return self.generate(component='desktop')

    def test_desktop_notices_include_exact_native_and_wrapper_licenses_and_binary_pin(self):
        body, manifest = self.generate_desktop()
        self.assertEqual(manifest['format'], DESKTOP_FORMAT)
        self.assertEqual(manifest['platform'], 'linux/amd64')
        self.assertEqual(manifest['backend'], 'llama')
        self.assertEqual(manifest['binarySha256'], sha256(self.binary.read_bytes()))
        self.assertIn(b'desktop host (linux/amd64, x86_64-unknown-linux-gnu)', body)
        self.assertIn(b'Named native license\n', body)
        self.assertIn(b'Embedded notice\n', body)
        self.assertIn(b'Pinned llama-cpp-rs-LICENSE-MIT.txt\n', body)
        self.assertIn(b'Pinned llama-cpp-rs-LICENSE-APACHE.txt\n', body)
        self.assertIn(b'Pinned rust-cuda-LICENSE-MIT.txt\n', body)
        self.assertIn(b'Pinned rust-cuda-LICENSE-APACHE.txt\n', body)
        self.assertNotIn(str(self.base).encode(), body)
        self.assertNotIn(str(self.base), json.dumps(manifest))
        self.assertEqual(self.generate_desktop(), (body, manifest))
        verify(body, manifest, component='desktop')
        with self.assertRaisesRegex(ValueError, 'unexpected Rust notice identity'):
            verify(body, manifest)

    def test_requires_successful_real_release_artifact(self):
        original = self.binary.read_bytes()
        self.binary.unlink()
        with self.assertRaisesRegex(ValueError, 'missing or not executable'):
            self.generate_desktop()
        self.binary.write_bytes(original)
        self.binary.chmod(0o755)
        with self.binary.open('r+b') as binary:
            binary.seek(18)
            binary.write(struct.pack('<H', 183))
        with self.assertRaisesRegex(ValueError, 'not a Linux/amd64 ELF executable'):
            self.generate_desktop()
        self.binary.write_bytes(original)
        self.records[0]['profile']['test'] = True
        with self.assertRaisesRegex(ValueError, 'expected the desktop release binary artifact'):
            self.generate_desktop()
        self.records[0]['profile']['test'] = False
        self.build_finished['success'] = False
        with self.assertRaisesRegex(ValueError, 'expected successful desktop release build'):
            self.generate_desktop()

    def test_rejects_stubs_and_requires_compiled_llama_libraries(self):
        workspace_records = [record for record in self.records if record.get('features') == ['llama']]
        for record in [self.records[0], *workspace_records]:
            features = record['features'][:]
            record['features'].append('stub')
            with self.assertRaisesRegex(ValueError, 'requires llama and excludes stub'):
                self.generate_desktop()
            record['features'] = features
        self.records[0]['features'] = ['default']
        with self.assertRaisesRegex(ValueError, 'requires llama and excludes stub'):
            self.generate_desktop()
        self.records[0]['features'] = ['default', 'llama']
        sys_crate = next(record for record in self.records
                         if record.get('target', {}).get('name') == 'llama-cpp-sys-2')
        sys_crate['target']['kind'] = ['custom-build']
        with self.assertRaisesRegex(ValueError, 'required compiled llama library artifacts absent'):
            self.generate_desktop()

    def test_rejects_missing_native_license_or_changed_wrapper_license(self):
        (self.base / 'cargo cache/llama-cpp-sys-2' / LLAMA_NATIVE_LICENSE).unlink()
        with self.assertRaisesRegex(ValueError, 'required compiled llama license text absent'):
            self.generate_desktop()
        self.write(f'cargo cache/llama-cpp-sys-2/{LLAMA_NATIVE_LICENSE}', b'Named native license\n')
        self.write('checkout/licenses/upstream/llama-cpp-rs-LICENSE-MIT.txt', b'Tampered\n')
        with self.assertRaisesRegex(ValueError, 'pinned supplemental notice changed'):
            self.generate_desktop()

    def test_verify_cli_checks_exact_release_binary(self):
        messages = self.write('build-messages.jsonl', ('\n'.join(map(json.dumps, self.records)) + '\n').encode())
        collect_args = ['notices', 'collect', '--component', 'desktop', '--build-messages', str(messages),
                        '--sysroot', str(self.sysroot), str(self.output)]
        verify_args = ['notices', 'verify', '--component', 'desktop', str(self.output)]
        with patch('make_flow_review_notices.ROOT', self.root):
            with patch('sys.argv', collect_args):
                main()
            with patch('sys.argv', verify_args):
                main()
                with self.binary.open('ab') as binary:
                    binary.write(b'changed')
                with self.assertRaisesRegex(ValueError, 'desktop release artifact differs'):
                    main()


if __name__ == '__main__':
    unittest.main()
