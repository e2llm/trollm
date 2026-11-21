import io
import json
import tarfile
import unittest
from unittest.mock import Mock, patch

from smoke import SmokeError, calculator_bundle, digest, start, wait_for_run


RUN_ID = "11111111-2222-4333-8444-555555555555"
FLOW_SHA = "a" * 64


class SmokeFixtureTests(unittest.TestCase):
    def test_archive_contains_exact_pins_and_proof(self):
        archive, sha = calculator_bundle()
        self.assertEqual(calculator_bundle(), (archive, sha))
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            self.assertTrue(all(member.isfile() for member in tar.getmembers()))
            manifest = json.load(tar.extractfile("manifest.json"))
            self.assertEqual(
                tar.getnames(),
                ["manifest.json"] + [item["path"] for item in manifest["files"]],
            )
            for item in manifest["files"]:
                self.assertEqual(digest(tar.extractfile(item["path"]).read()), item["sha256"])
            plan = tar.extractfile(manifest["entryFlow"]).read()
            self.assertEqual(digest(plan), sha)
            self.assertIn(digest(tar.extractfile("setup.json").read()).encode(), plan)
            tasks = tar.extractfile(manifest["taskRegistry"]).read()
            self.assertIn(b"fromVars: {left: calculate.outputs.calculation.result}", tasks)
            self.assertIn(b"fixed: {right: '80.09'}", tasks)


class SmokeProtocolTests(unittest.TestCase):
    def test_duplicate_start_uses_the_original_request_id_and_body(self):
        client = Mock()
        client.json.side_effect = [{"id": RUN_ID}, {"id": RUN_ID}]
        self.assertEqual(start(client, FLOW_SHA, "", check_repeat=True), RUN_ID)
        first, second = client.json.call_args_list
        self.assertEqual(first, second)
        self.assertEqual(first.args, ("/api/runs",))
        self.assertEqual(first.kwargs["fields"]["flowSha256"], FLOW_SHA)
        self.assertEqual(len(first.kwargs["fields"]["requestId"]), 36)

    def test_run_drains_after_succeeded_until_cursor_is_complete(self):
        client = Mock()
        run = {"id": RUN_ID, "flowSha256": FLOW_SHA, "phase": "succeeded", "workerActive": False}
        client.json.side_effect = [
            {"run": run, "events": [{"seq": 1, "type": "direct_tool_step"}], "hasMore": True, "nextSeq": 1},
            {"run": run, "events": [{"seq": 2, "type": "artifact_written"}], "hasMore": False, "nextSeq": 2},
        ]
        with patch("smoke.time.sleep"):
            self.assertEqual(wait_for_run(client, RUN_ID, FLOW_SHA, 5),
                             (run, {"direct_tool_step": 1, "artifact_written": 1}))
        self.assertEqual([call.args[0].rsplit("=", 1)[1] for call in client.json.call_args_list], ["0", "1"])

    def test_run_refuses_nonadvancing_cursor(self):
        client = Mock()
        run = {"id": RUN_ID, "flowSha256": FLOW_SHA, "phase": "running", "workerActive": True}
        client.json.return_value = {"run": run, "events": [], "hasMore": True, "nextSeq": 0}
        with self.assertRaisesRegex(SmokeError, "stopped advancing"):
            wait_for_run(client, RUN_ID, FLOW_SHA, 5)
        client.json.assert_called_once()


if __name__ == "__main__":
    unittest.main()
