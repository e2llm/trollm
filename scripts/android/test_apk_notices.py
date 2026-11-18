import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from make_apk_notices import collect, combine_metadata, runtime_packages, rust_toolchain_notices


class ApkNoticesTest(unittest.TestCase):
    def metadata(self, directory):
        packages = [
            {'id': 'app', 'name': 'trollm-uniffi', 'source': None, 'version': '1.0.0'},
            {'id': 'runtime', 'name': 'fixture-runtime', 'source': 'fixture', 'version': '1',
             'manifest_path': str(directory / 'Cargo.toml'), 'license': 'MIT'},
            {'id': 'dev', 'name': 'fixture-dev', 'source': 'fixture', 'version': '1'},
        ]
        nodes = [{'id': 'app', 'deps': [
            {'pkg': 'runtime', 'dep_kinds': [{'kind': None}]},
            {'pkg': 'dev', 'dep_kinds': [{'kind': 'dev'}]},
        ]}, {'id': 'runtime', 'deps': []}, {'id': 'dev', 'deps': []}]
        return {'packages': packages, 'resolve': {'nodes': nodes}}

    def test_runtime_graph_excludes_dev_only_packages(self):
        self.assertEqual([p['name'] for p in runtime_packages(self.metadata(Path('/fixture')))],
                         ['fixture-runtime'])

    def test_architecture_specific_runtime_dependencies_are_retained(self):
        first = self.metadata(Path('/fixture'))
        second = json.loads(json.dumps(first))
        second['resolve']['nodes'][0]['deps'][1]['dep_kinds'][0]['kind'] = None
        combined = combine_metadata([first, second])
        self.assertEqual([p['name'] for p in runtime_packages(combined)],
                         ['fixture-dev', 'fixture-runtime'])

    def write_sysroot_notices(self, sysroot):
        copyright_file = sysroot / 'share/doc/rust/COPYRIGHT-library.html'
        terms = copyright_file.parent / 'licenses'
        terms.mkdir(parents=True)
        copyright_file.write_bytes(b'<p>Exact standard-library attribution.</p>\n')
        for name in ('Apache-2.0', 'MIT', 'LLVM-exception'):
            (terms / (name + '.txt')).write_text(name + ' terms fixture\n')
        return copyright_file, terms

    def test_missing_or_empty_toolchain_notices_fail(self):
        with tempfile.TemporaryDirectory() as folder:
            sysroot = Path(folder)
            copyright_file, terms = self.write_sysroot_notices(sysroot)
            copyright_file.unlink()
            with self.assertRaisesRegex(ValueError, 'standard-library copyright or license texts unavailable'):
                list(rust_toolchain_notices(sysroot))
            copyright_file.write_text('Copyright notice')
            (terms / 'LLVM-exception.txt').write_text(' ')
            with self.assertRaisesRegex(ValueError, 'empty Rust toolchain notice'):
                list(rust_toolchain_notices(sysroot))
            (terms / 'LLVM-exception.txt').unlink()
            with self.assertRaisesRegex(ValueError, 'standard-library copyright or license texts unavailable'):
                list(rust_toolchain_notices(sysroot))

    def test_notices_preserve_exact_license_and_record_matching_digest(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'LICENSE').write_text('Synthetic project license fixture\n')
            (root / 'THIRD_PARTY_NOTICES.md').write_text('Synthetic dependency guide\n')
            (root / 'Cargo.toml').write_text('[workspace.package]\nversion="1.2.3"\n')
            supplemental = root / 'licenses/upstream'
            supplemental.mkdir(parents=True)
            (supplemental / 'sources.json').write_text('[]')
            package = root / 'package'
            package.mkdir()
            original = b'Synthetic runtime license: retain this complete text.\n'
            (package / 'LICENSE').write_bytes(original)
            ndk = root / 'ndk'
            ndk.mkdir()
            (ndk / 'NOTICE').write_text('Synthetic NDK notice\n')
            android = {'components': [{'coordinate': 'fixture:android:1',
                'licenses': [{'name': 'Apache-2.0', 'url': 'https://example.org/license'}],
                'notices': []}]}
            output = root / 'output'
            sysroot = root / 'private rust toolchain'
            copyright_file, _ = self.write_sysroot_notices(sysroot)
            collect(root, self.metadata(package), android, ndk, output, sysroot)
            text = (output / 'THIRD_PARTY_NOTICES.txt').read_bytes()
            manifest = json.loads((output / 'third-party-manifest.json').read_text())
            self.assertIn(original, text)
            self.assertIn(copyright_file.read_bytes(), text)
            notice = next(x for x in manifest['files'] if x['name'].endswith('/COPYRIGHT-library.html'))
            self.assertEqual(notice['source'], 'Installed Rust toolchain: share/doc/rust/COPYRIGHT-library.html')
            self.assertEqual(notice['sha256'], hashlib.sha256(copyright_file.read_bytes()).hexdigest())
            self.assertNotIn(str(sysroot), json.dumps(manifest))
            self.assertNotIn(str(sysroot).encode(), text)
            self.assertEqual(manifest['noticesSha256'], hashlib.sha256(text).hexdigest())
            self.assertTrue(any(x['sha256'] == hashlib.sha256(original).hexdigest()
                                for x in manifest['files']))
            (package / 'LICENSE').unlink()
            with self.assertRaisesRegex(ValueError, 'missing Rust license text'):
                collect(root, self.metadata(package), android, ndk, output, sysroot)


if __name__ == '__main__':
    unittest.main()
