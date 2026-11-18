"""Platform path construction must not change archive names or source bytes."""

import hashlib
from pathlib import Path, PurePosixPath, PureWindowsPath
import tempfile
import unittest
from unittest.mock import patch

from bundle_paths import archive_path, read_bundle_files, registry_prompt_paths
import make_starter_bundles as starter


class BundlePaths(unittest.TestCase):
    def test_only_model_tasks_supply_prompt_payloads(self):
        registry = {"tasks": {
            "direct": {"execution": "tool"},
            "legacy": {"model": "coder", "prompt": "prompts/task.md"},
            "model": {"execution": "model", "model": "reasoner", "prompt": "prompts/task.md"},
        }}
        self.assertEqual(registry_prompt_paths(registry, "tasks/registry.tasks.yaml"),
                         ["tasks/prompts/task.md"])
        for field, value in (("model", "coder"), ("prompt", "missing.md"),
                             ("maxOutputTokens", None), ("generationTimeoutS", 1), ("loop", {})):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "direct tool"):
                registry_prompt_paths({"tasks": {"direct": {"execution": "tool", field: value}}},
                                      "registry.tasks.yaml")
        for task in ({"model": "coder"}, {"prompt": "task.md"},
                     {"execution": "unknown"}, {"model": "coder", "prompt": "../task.md"}):
            with self.subTest(task=task), self.assertRaises(ValueError):
                registry_prompt_paths({"tasks": {"bad": task}}, "registry.tasks.yaml")

    def test_windows_and_posix_sources_build_identical_starters(self):
        for name, (source, asset, names) in starter.STARTERS.items():
            for path_type in (PureWindowsPath, PurePosixPath):
                with self.subTest(starter=name, platform=path_type.__name__):
                    with patch.dict(starter.STARTERS, {
                        name: (source, asset, tuple(path_type(p) for p in names))
                    }):
                        self.assertEqual(starter.build_bundle(starter=name), asset.read_bytes())

    def test_bytes_are_not_normalized_and_duplicate_or_escaping_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "prompts").mkdir()
            original = b"Exact text\r\nwith original endings\r\n"
            (root / "prompts/task.md").write_bytes(original)
            windows = PureWindowsPath("prompts/task.md")
            files = read_bundle_files(root, [windows])
            self.assertEqual(files, {"prompts/task.md": original})
            self.assertEqual(hashlib.sha256(files["prompts/task.md"]).digest(),
                             hashlib.sha256(original).digest())
            with self.assertRaisesRegex(ValueError, "Duplicate"):
                read_bundle_files(root, [windows, "prompts/task.md"])
            (root / "source").mkdir()
            (root / "source/outside.md").symlink_to(root / "prompts/task.md")
            with self.assertRaisesRegex(ValueError, "escapes"):
                read_bundle_files(root / "source", ["outside.md"])

    def test_unsafe_archive_names_are_not_repaired(self):
        for path in ("", "../task.md", "/task.md", "a/../task.md", "a//b",
                     "a/./b", "a\\b", "C:task.md", "C:/task.md", "bad\0name",
                     PureWindowsPath("C:/task.md"), PureWindowsPath("/task.md")):
            with self.subTest(path=path), self.assertRaises(ValueError):
                archive_path(path)


if __name__ == "__main__":
    unittest.main()
