"""Offline identity, schema and binding checks for the small paired probe."""

import contextlib
import hashlib
import io
import json
from pathlib import PurePosixPath
import tarfile
import tempfile
import unittest

from jsonschema import Draft202012Validator
import make_paired_probe_bundle as fixture


class PairedProbeBundle(unittest.TestCase):
    def unpack(self):
        with tarfile.open(fileobj=io.BytesIO(fixture.build_bundle()), mode="r:") as archive:
            return {member.name: archive.extractfile(member).read() for member in archive}

    def test_small_deterministic_ustar_and_manifest_pins(self):
        data = fixture.build_bundle()
        self.assertEqual(data, fixture.build_bundle())
        self.assertLess(len(data), 32 * 1024)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            self.assertEqual(len(archive.getmembers()), 6)
            for member in archive:
                self.assertTrue(member.isfile())
                self.assertFalse(PurePosixPath(member.name).is_absolute())
                self.assertNotIn("..", PurePosixPath(member.name).parts)
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
        files = self.unpack()
        manifest = json.loads(files.pop("manifest.json"))
        self.assertEqual({entry["path"] for entry in manifest["files"]}, set(files))
        for entry in manifest["files"]:
            self.assertEqual(entry["sha256"], hashlib.sha256(files[entry["path"]]).hexdigest())
        plan = json.loads(files[manifest["entryFlow"]])
        self.assertEqual(manifest["entryFlow"], plan["id"] + ".flow.yaml")
        self.assertEqual(plan["setup"]["sha256"], hashlib.sha256(files[plan["setup"]["path"]]).hexdigest())
        changed = files[fixture.SETUP_PATH] + b"\n"
        self.assertNotEqual(plan["setup"]["sha256"], hashlib.sha256(changed).hexdigest())

    def test_plan_and_setup_validate_with_current_schemas(self):
        files = self.unpack()
        plan = json.loads(files[f"{fixture.FLOW_ID}.flow.yaml"])
        setup = json.loads(files[fixture.SETUP_PATH])
        for name, value in (("execution-plan.v1.schema.json", plan), ("flow-setup.v1.schema.json", setup)):
            schema = json.loads((fixture.REPO / "schemas" / name).read_text())
            Draft202012Validator(schema).validate(value)
        self.assertEqual(set(plan["models"]), {"coder", "reasoner"})
        self.assertEqual(setup["connections"], {})
        self.assertEqual(plan["mcp"], {"servers": [], "policies": {}})
        self.assertEqual(plan["limits"], {"maxSteps": 2, "maxReasonerCalls": 1,
            "maxTokensTotal": 8192, "timeoutS": 600, "artifactMaxMb": 1})
        self.assertEqual(plan["onLimit"], "abort")

    def test_task_refs_and_exact_coder_prose_handoff_are_tool_free(self):
        files = self.unpack()
        plan = json.loads(files[f"{fixture.FLOW_ID}.flow.yaml"])
        registry = json.loads(files[f"{fixture.FLOW_ID}.tasks.yaml"])
        self.assertEqual(registry["id"], fixture.FLOW_ID)
        self.assertEqual(plan["entryStep"], "draft")
        for node_id, slot in (("draft", "coder"), ("summarize", "reasoner")):
            task = registry["tasks"][plan["nodes"][node_id]["taskRef"]]
            self.assertEqual(task["model"], slot)
            self.assertEqual(task["tools"], [])
            self.assertEqual(task["loop"], {"maxSteps": 1})
            self.assertEqual(task["outputContract"]["outputs"], {"report":{"from":"model","type":"prose"}})
            self.assertIn(task["prompt"], files)
        self.assertEqual(plan["nodes"]["draft"]["onVerified"], "summarize")
        self.assertEqual(plan["nodes"]["summarize"]["onVerified"], "done")
        self.assertEqual(plan["nodes"]["done"], {"kind":"end","outcome":"succeeded"})
        self.assertIn("{{ user_input }}", files["prompts/answer.md"].decode())
        reasoner = files["prompts/summarize.md"].decode()
        self.assertIn("{{ draft.outputs.report }}", reasoner)
        self.assertNotIn("{{ user_input }}", reasoner)
        self.assertNotIn("READY", reasoner)

    def test_exact_candidate_model_pins_and_sampling(self):
        setup = json.loads(self.unpack()[fixture.SETUP_PATH])
        expected = {
            "coder": ("bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517", 532517120,
                "ed4bdcdbcce9f095bb49e8759176c64e45fa294b/Qwen3.5-0.8B-Q4_K_M.gguf"),
            "reasoner": ("740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8", 3106738272,
                "0314792d7f1f7e229411f620751375812bb9faf2/gemma-4-E2B-it-Q4_K_M.gguf"),
        }
        for slot, (hash_, size, suffix) in expected.items():
            model = setup["models"][slot]
            self.assertEqual(model["sha256"], hash_)
            self.assertEqual(model["sizeBytes"], size)
            self.assertTrue(model["url"].startswith("https://huggingface.co/unsloth/"))
            self.assertTrue(model["url"].endswith("/resolve/" + suffix))
            self.assertEqual((model["ctx"], model["temperature"], model["topP"]), (4096, 0.2, 0.95))

    def test_main_writes_requested_output(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            path = fixture.Path(directory) / "probe.tar"
            self.assertEqual(fixture.main(["--output", str(path)]), 0)
            self.assertEqual(path.read_bytes(), fixture.build_bundle())


if __name__ == "__main__":
    unittest.main()
