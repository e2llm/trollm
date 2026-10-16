"""Offline checks for the shipped starter's source, pins and reproducible asset."""

import contextlib
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import unittest

from jsonschema import Draft202012Validator
import yaml
import make_starter_bundles as fixture


class StarterBundles(unittest.TestCase):
    def unpack(self, starter="text-summary"):
        with tarfile.open(fileobj=io.BytesIO(fixture.build_bundle(starter=starter)), mode="r:") as archive:
            return {member.name: archive.extractfile(member).read() for member in archive}

    def test_committed_asset_is_small_reproducible_and_manifest_verified(self):
        data = fixture.build_bundle()
        self.assertEqual(data, fixture.build_bundle())
        self.assertEqual(fixture.ASSET.read_bytes(), data)
        self.assertLess(len(data), 32 * 1024)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            self.assertEqual(len(archive.getmembers()), 5)
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
        flow = yaml.safe_load(files[manifest["entryFlow"]])
        self.assertEqual(flow["id"] + ".flow.yaml", manifest["entryFlow"])
        self.assertEqual(flow["setup"]["sha256"], hashlib.sha256(files[flow["setup"]["path"]]).hexdigest())

    def test_current_schemas_model_defaults_and_tool_free_references(self):
        files = self.unpack()
        plan = yaml.safe_load(files["text-summary.flow.yaml"])
        setup = json.loads(files["setup.json"])
        for name, value in (("execution-plan.v1.schema.json", plan), ("flow-setup.v1.schema.json", setup)):
            Draft202012Validator(json.loads((fixture.REPO / "schemas" / name).read_text())).validate(value)
        self.assertEqual(setup["title"], "Summarize text")
        self.assertEqual(setup["connections"], {})
        self.assertEqual(plan["models"], {"coder":{"ctx":4096}})
        model = setup["models"]["coder"]
        self.assertEqual(model["sha256"], "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517")
        self.assertEqual(model["sizeBytes"], 532517120)
        self.assertIn("/resolve/ed4bdcdbcce9f095bb49e8759176c64e45fa294b/Qwen3.5-0.8B-Q4_K_M.gguf", model["url"])
        self.assertEqual((model["ctx"], model["temperature"], model["topP"]), (4096,0.2,0.95))
        self.assertEqual(plan["mcp"], {"servers":[],"policies":{}})
        registry = yaml.safe_load(files["text-summary.tasks.yaml"])
        node = plan["nodes"][plan["entryStep"]]
        task = registry["tasks"][node["taskRef"]]
        self.assertEqual(task["model"], "coder")
        self.assertEqual(task["tools"], [])
        self.assertEqual(task["loop"], {"maxSteps":1})
        self.assertEqual(task["outputContract"]["outputs"]["report"], {"from":"model","type":"prose"})
        self.assertIn("{{ user_input }}", files[task["prompt"]].decode())
        self.assertEqual(plan["nodes"][node["onVerified"]], {"kind":"end","outcome":"succeeded"})

    def test_changed_setup_requires_updating_root_flow_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source"
            shutil.copytree(fixture.SOURCE, source)
            path = source / "setup.json"
            path.write_bytes(path.read_bytes() + b"\n")
            with self.assertRaisesRegex(ValueError, "root flow"):
                fixture.build_bundle(source)

    def test_cli_check_does_not_modify_matching_asset(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(fixture.main(["--check"]), 0)

    def test_browser_asset_manifest_schemas_and_defaults(self):
        from make_paired_probe_bundle import MODELS
        data = fixture.build_bundle(starter="browser-summary")
        self.assertEqual(data, fixture.BROWSER_ASSET.read_bytes())
        self.assertEqual(data, fixture.build_bundle(starter="browser-summary"))
        self.assertLess(len(data), 64 * 1024)
        files = self.unpack("browser-summary")
        manifest = json.loads(files.pop("manifest.json"))
        self.assertEqual(set(files), set(fixture.BROWSER_FILES))
        self.assertEqual({entry["path"] for entry in manifest["files"]}, set(files))
        for entry in manifest["files"]:
            self.assertEqual(entry["sha256"], hashlib.sha256(files[entry["path"]]).hexdigest())
        plan = yaml.safe_load(files[manifest["entryFlow"]])
        setup = json.loads(files["setup.json"])
        registry = yaml.safe_load(files[manifest["taskRegistry"]])
        for schema, value in (("execution-plan.v1.schema.json", plan),
                              ("flow-setup.v1.schema.json", setup)):
            Draft202012Validator(json.loads((fixture.REPO / "schemas" / schema).read_text())).validate(value)
        self.assertEqual(plan["setup"]["sha256"], hashlib.sha256(files["setup.json"]).hexdigest())
        self.assertEqual(setup["models"], MODELS)
        self.assertEqual(setup["title"], "Browse and summarize")
        self.assertEqual(setup["connections"]["e2llm"]["auth"], "oauth")
        self.assertEqual([field["key"] for field in setup["input"]["fields"]], ["browser", "url", "question"])
        self.assertEqual(setup["input"]["toolArguments"], {
            "e2llm.list_tabs": {"fromInput": {"browser": "browser"}},
            "e2llm.act": {"fromInput": {"browser": "browser", "target": "url"},
                          "fixed": {"action": "open"}, "forbid": ["sessionId", "tabId"]},
            "e2llm.read_page": {"fromInput": {"browser": "browser"},
                                "require": ["tabId"], "forbid": ["sessionId", "selector"]},
        })
        tasks = registry["tasks"]
        for task in tasks.values():
            self.assertIn(task["prompt"], files)
        self.assertEqual([tasks[name]["loop"]["maxSteps"] for name in
                          ("open-page", "read-page", "summarize-page")], [2, 3, 1])
        self.assertEqual(tasks["open-page"]["tools"], ["e2llm.act"])
        self.assertNotIn("allowNoop", tasks["open-page"]["outputContract"])
        self.assertEqual(tasks["open-page"]["mutations"], {"idempotency": "intent-log", "onUncertain": "fail-run"})
        self.assertEqual(tasks["summarize-page"]["tools"], [])
        self.assertEqual(tasks["summarize-page"]["model"], "reasoner")
        self.assertIn("{{ read_page.outputs.page }}", files["prompts/summarize-page.md"].decode())
        self.assertNotIn("outputs.report", files["prompts/summarize-page.md"].decode())
        self.assertEqual(tasks["read-page"]["outputContract"], {
            "outputs": {"page": {"fromArtifact": "page", "format": "json", "type": "object"}},
            "requireArtifactComplete": ["page"],
        })
        policies = plan["mcp"]["policies"]
        self.assertTrue(all("retry" not in p and "cache" not in p for p in policies.values()))
        self.assertEqual(policies["e2llm.read_page"]["shape"], {"strip": ["content"], "toFile": True})
        validator = Draft202012Validator(policies["e2llm.read_page"]["resultValidation"]["schema"])
        page = {"content": "Exact page", "url": "https://example.org/", "stats": {"contentNodes": 1, "truncated": False}}
        validator.validate(page)
        for invalid in ({**page, "content": ""}, {**page, "selectorUnmatched": True},
                        {**page, "stats": {"contentNodes": 1, "truncated": True}}):
            self.assertFalse(validator.is_valid(invalid))

    def test_both_assets_are_installed_and_explicit_output_retains_text_default(self):
        source = (fixture.REPO / "apps/android/app/src/main/java/com/trollm/app/StarterFlows.kt").read_text()
        for name in fixture.STARTERS:
            self.assertIn(f'"flows/{name}.tar"', source)
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            path = Path(directory) / "starter.tar"
            fixture.main(["--output", str(path)])
            self.assertEqual(path.read_bytes(), fixture.build_bundle())


if __name__ == "__main__":
    unittest.main()
