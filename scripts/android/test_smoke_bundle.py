"""Offline fixture contracts; no SDK, Rust build, device, or network required."""

import hashlib
import io
import json
from pathlib import PurePosixPath
import tarfile
import unittest

import make_smoke_bundle as fixture


def unpack(data):
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
        return {member.name: archive.extractfile(member).read() for member in archive}


class SmokeBundleContract(unittest.TestCase):
    def test_reproducible_plain_ustar_with_safe_regular_members(self):
        first = fixture.build_bundle()
        self.assertEqual(first, fixture.build_bundle())
        self.assertLess(len(first), 32 * 1024)
        with tarfile.open(fileobj=io.BytesIO(first), mode="r:") as archive:
            members = archive.getmembers()
            self.assertEqual(len(members), 4)
            self.assertEqual(len({item.name for item in members}), len(members))
            for item in members:
                self.assertTrue(item.isfile())
                self.assertFalse(PurePosixPath(item.name).is_absolute())
                self.assertNotIn("..", PurePosixPath(item.name).parts)
                self.assertEqual((item.uid, item.gid, item.mtime), (0, 0, 0))
                self.assertEqual(first[item.offset + 257:item.offset + 263], b"ustar\0")

    def test_manifest_authenticates_every_payload_and_root_identity(self):
        files = unpack(fixture.build_bundle())
        manifest = json.loads(files.pop("manifest.json"))
        self.assertEqual(manifest["format"], "trollm.flow-bundle.v1")
        self.assertEqual({item["path"] for item in manifest["files"]}, set(files))
        self.assertEqual(len(manifest["files"]), len(files))
        for item in manifest["files"]:
            self.assertEqual(hashlib.sha256(files[item["path"]]).hexdigest(), item["sha256"])
        plan = json.loads(files[manifest["entryFlow"]])
        self.assertEqual(plan["id"], "android-smoke")
        self.assertEqual(manifest["entryFlow"], plan["id"] + ".flow.yaml")
        self.assertEqual(len(plan["sourceBaseline"]["sha256"]), 64)

    def test_corrupted_fixture_changes_only_one_manifest_digest(self):
        good = unpack(fixture.build_bundle())
        bad = unpack(fixture.build_bundle(corrupt_hash=True))
        self.assertEqual(set(good), set(bad))
        for path in good.keys() - {"manifest.json"}:
            self.assertEqual(good[path], bad[path])
        manifest = json.loads(bad["manifest.json"])
        mismatches = [item["path"] for item in manifest["files"]
                      if hashlib.sha256(bad[item["path"]]).hexdigest() != item["sha256"]]
        self.assertEqual(mismatches, [fixture.PROMPT_FILE])

    def test_bounded_tool_free_task_reaches_success_and_exposes_report(self):
        files = unpack(fixture.build_bundle())
        plan = json.loads(files[fixture.FLOW_FILE])
        registry = json.loads(files[fixture.REGISTRY_FILE])
        self.assertEqual(plan["mcp"], {"servers": [], "policies": {}})
        self.assertEqual(plan["models"], {"coder": {"ctx": 2048}})
        node = plan["nodes"][plan["entryStep"]]
        self.assertEqual(node["kind"], "task")
        task = registry["tasks"][node["taskRef"]]
        self.assertEqual(task["tools"], [])
        self.assertEqual(task["loop"]["maxSteps"], 1)
        self.assertIn(task["prompt"], files)
        self.assertIn("TEST ONLY", files[task["prompt"]].decode())
        self.assertEqual(task["outputContract"]["outputs"]["report"],
                         {"from": "model", "type": "prose"})
        self.assertEqual(plan["nodes"][node["onVerified"]], {"kind": "end", "outcome": "succeeded"})
        self.assertGreaterEqual(plan["limits"]["maxSteps"], 2)
        self.assertLessEqual(plan["limits"]["timeoutS"], 30)

    def test_pausing_fixture_is_bounded_and_cannot_execute_remote_tools(self):
        files = unpack(fixture.build_bundle(paused=True))
        manifest = json.loads(files["manifest.json"])
        plan = json.loads(files[manifest["entryFlow"]])
        registry = json.loads(files[manifest["taskRegistry"]])
        self.assertEqual(plan["id"], "android-smoke-paused")
        self.assertEqual(manifest["entryFlow"], plan["id"] + ".flow.yaml")
        self.assertEqual(plan["mcp"], {"servers": [], "policies": {}})
        node = plan["nodes"][plan["entryStep"]]
        self.assertEqual(node["onError"], {
            "retry": 0, "then": {"status": "paused", "reason": "needs-attention"}})
        task = registry["tasks"][node["taskRef"]]
        self.assertEqual(task["tools"], ["srv.tool"])
        self.assertFalse(task["outputContract"]["allowNoop"])
        self.assertEqual(task["loop"]["maxSteps"], 1)
        self.assertLessEqual(plan["limits"]["timeoutS"], 30)
        self.assertEqual(fixture.build_bundle(paused=True), fixture.build_bundle(paused=True))
        for item in manifest["files"]:
            self.assertEqual(hashlib.sha256(files[item["path"]]).hexdigest(), item["sha256"])


if __name__ == "__main__":
    unittest.main()
