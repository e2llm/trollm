import json
from pathlib import Path
import tempfile
import unittest

from make_flow_review_notices import (CRATES_IO, MANIFEST_NAME, SERVER, TARGET,
                                     TEXT_NAME, collect, sha256, verify)


class FlowReviewNoticesTest(unittest.TestCase):
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

    def generate(self):
        collect(self.root, map(json.dumps, self.records), self.sysroot, self.output)
        return ((self.output / TEXT_NAME).read_bytes(),
                json.loads((self.output / MANIFEST_NAME).read_text()))

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


if __name__ == '__main__':
    unittest.main()
