"""Offline native probe safety, bundle integrity and existing v1 schema checks."""
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
import tempfile
import unittest

from jsonschema import Draft202012Validator
import make_native_probe_bundle as fixture

SCHEMA = {"type": "object", "properties": {"browser": {"type": "string"}}}


class NativeProbeBundle(unittest.TestCase):
    def unpack(self, ready=False, conditional=False, observed=False):
        data = fixture.build_bundle(ready=ready, conditional=conditional, observed=observed,
                                    tool_schema=SCHEMA)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            return {item.name: archive.extractfile(item).read() for item in archive}

    def test_reproducible_small_safe_ustar_and_authenticated_payloads(self):
        for ready, conditional, observed in ((False, False, False), (True, False, False),
                                              (False, True, False), (False, False, True)):
            with self.subTest(ready=ready, conditional=conditional, observed=observed):
                data = fixture.build_bundle(ready=ready, conditional=conditional,
                                            observed=observed, tool_schema=SCHEMA)
                self.assertEqual(data, fixture.build_bundle(ready=ready, conditional=conditional,
                                                            observed=observed, tool_schema=SCHEMA))
                self.assertLess(len(data), 32 * 1024)
                with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
                    self.assertEqual(len(archive.getmembers()), 4)
                    for item in archive:
                        self.assertTrue(item.isfile())
                        self.assertFalse(PurePosixPath(item.name).is_absolute())
                        self.assertNotIn("..", PurePosixPath(item.name).parts)
                        self.assertEqual((item.uid, item.gid, item.mtime), (0, 0, 0))
                files = self.unpack(ready, conditional, observed)
                manifest = json.loads(files.pop("manifest.json"))
                self.assertEqual({item["path"] for item in manifest["files"]}, set(files))
                for item in manifest["files"]:
                    self.assertEqual(item["sha256"], hashlib.sha256(files[item["path"]]).hexdigest())

    def test_all_plans_validate_with_existing_schema_and_resolve_task_and_prompt(self):
        validator = Draft202012Validator(json.loads(
            (fixture.REPO / "schemas/execution-plan.v1.schema.json").read_text()))
        for ready, conditional, observed in ((False, False, False), (True, False, False),
                                              (False, True, False), (False, False, True)):
            files = self.unpack(ready, conditional, observed)
            manifest = json.loads(files["manifest.json"])
            plan = json.loads(files[manifest["entryFlow"]])
            validator.validate(plan)
            self.assertNotIn("sourceBaseline", plan)
            self.assertEqual(plan["models"], {"coder": {"ctx": 4096}})
            self.assertEqual(plan["limits"], {"maxSteps": 2, "maxReasonerCalls": 0,
                                            "maxTokensTotal": 8192, "timeoutS": 180,
                                            "artifactMaxMb": 1})
            self.assertEqual(plan["onLimit"], "abort")
            registry = json.loads(files[manifest["taskRegistry"]])
            self.assertEqual(registry["schemaVersion"], "ledger.task-registry.v1")
            node = plan["nodes"][plan["entryStep"]]
            task = registry["tasks"][node["taskRef"]]
            self.assertEqual(task["model"], "coder")
            self.assertIn(task["prompt"], files)
            self.assertNotIn("/no_think", files[task["prompt"]].decode())
            self.assertNotIn("/nothink", files[task["prompt"]].decode())
            self.assertEqual(task["outputContract"]["outputs"]["report"],
                             {"from": "model", "type": "prose"})
            self.assertEqual(plan["nodes"][node["onVerified"]], {"kind": "end", "outcome": "succeeded"})

    def test_mcp_probe_grants_only_list_tabs_and_embeds_only_its_schema(self):
        files = self.unpack()
        plan = json.loads(files[f"{fixture.MCP_FLOW_ID}.flow.yaml"])
        task = json.loads(files[f"{fixture.MCP_FLOW_ID}.tasks.yaml"])["tasks"]["probe"]
        self.assertEqual(plan["mcp"], {"servers": [{"name": "e2llm", "url": "https://mcp.e2llm.com/mcp"}],
                                       "policies": {"e2llm.list_tabs": {"class": "read", "timeoutS": 20}}})
        self.assertEqual(task["tools"], ["e2llm.list_tabs"])
        self.assertEqual(task["loop"]["maxSteps"], 2)
        prompt = files[task["prompt"]].decode()
        self.assertIn('<tool_call>{"server":"e2llm","tool":"list_tabs","args":{}}</tool_call>', prompt)
        self.assertIn(json.dumps(SCHEMA, sort_keys=True, indent=2), prompt)
        self.assertIn("untrusted data", prompt)
        self.assertIn("number of tabs returned", prompt)

    def test_ready_probe_is_tool_free_and_one_generation(self):
        files = self.unpack(ready=True)
        plan = json.loads(files[f"{fixture.READY_FLOW_ID}.flow.yaml"])
        task = json.loads(files[f"{fixture.READY_FLOW_ID}.tasks.yaml"])["tasks"]["probe"]
        self.assertEqual(plan["mcp"], {"servers": [], "policies": {}})
        self.assertEqual(task["tools"], [])
        self.assertEqual(task["loop"]["maxSteps"], 1)
        self.assertIn("Reply exactly READY", files[task["prompt"]].decode())

    def test_conditional_probe_changes_only_identity_and_phase_instructions(self):
        files = self.unpack(conditional=True)
        original = self.unpack()
        for suffix in ("flow.yaml", "tasks.yaml"):
            value = json.loads(files[f"{fixture.CONDITIONAL_FLOW_ID}.{suffix}"])
            self.assertEqual(value["id"], fixture.CONDITIONAL_FLOW_ID)
            value["id"] = fixture.MCP_FLOW_ID
            self.assertEqual(value, json.loads(original[f"{fixture.MCP_FLOW_ID}.{suffix}"]))
        registry = json.loads(files[f"{fixture.CONDITIONAL_FLOW_ID}.tasks.yaml"])
        task = registry["tasks"]["probe"]
        self.assertEqual(task["tools"], ["e2llm.list_tabs"])
        self.assertEqual(task["loop"]["maxSteps"], 2)
        self.assertFalse(task["outputContract"].get("allowNoop", False))
        prompt = files[task["prompt"]].decode()
        self.assertIn("If nothing follows that line, output exactly this text:", prompt)
        self.assertIn('<tool_call>{"server":"e2llm","tool":"list_tabs","args":{}}</tool_call>', prompt)
        self.assertIn("If a tool result follows that line", prompt)
        self.assertIn(json.dumps(SCHEMA, sort_keys=True, indent=2), prompt)
        self.assertTrue(prompt.endswith("\nEND OF INSTRUCTIONS\n"))

    def test_observation_changes_only_identity_and_zero_ttl_cache(self):
        original = self.unpack(conditional=True)
        observed = self.unpack(observed=True)
        self.assertEqual(observed[fixture.PROMPT_FILE], original[fixture.PROMPT_FILE])
        for suffix in ("flow.yaml", "tasks.yaml"):
            value = json.loads(observed[f"{fixture.OBSERVED_FLOW_ID}.{suffix}"])
            value["id"] = fixture.CONDITIONAL_FLOW_ID
            if suffix == "flow.yaml":
                self.assertEqual(value["mcp"]["policies"]["e2llm.list_tabs"].pop("cache"), {"ttlS": 0})
            self.assertEqual(value, json.loads(original[f"{fixture.CONDITIONAL_FLOW_ID}.{suffix}"]))

    def test_probe_modes_conflict(self):
        for build in (fixture.fixture_files, fixture.build_bundle):
            for modes in ({"ready": True, "conditional": True}, {"ready": True, "observed": True},
                          {"conditional": True, "observed": True}):
                with self.assertRaisesRegex(ValueError, "mutually exclusive"):
                    build(**modes, tool_schema=SCHEMA)

    def test_schema_loader_selects_unique_tool_without_embedding_snapshot_data(self):
        with tempfile.TemporaryDirectory() as folder:
            snapshot = Path(folder) / "tools.json"
            snapshot.write_text(json.dumps({"tools": [
                {"name": "list_tabs", "inputSchema": SCHEMA, "ignored": "PRIVATE"},
                {"name": "another_tool", "inputSchema": {"type": "string"}},
            ]}))
            self.assertEqual(fixture.list_tabs_schema(snapshot), SCHEMA)
            for tools in ([], [{"name": "list_tabs"}],
                          [{"name": "list_tabs", "inputSchema": SCHEMA}] * 2):
                snapshot.write_text(json.dumps({"tools": tools}))
                with self.assertRaises(ValueError):
                    fixture.list_tabs_schema(snapshot)


if __name__ == "__main__":
    unittest.main()
