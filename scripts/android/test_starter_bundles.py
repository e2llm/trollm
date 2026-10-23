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
            "e2llm.act": {"fromInput": {"browser": "browser", "target": "url"},
                          "fixed": {"action": "open"}, "forbid": ["sessionId", "tabId"]},
            "e2llm.read_page": {"fromInput": {"browser": "browser"},
                                "require": ["tabId"], "forbid": ["sessionId", "selector"]},
        })
        tasks = registry["tasks"]
        self.assertEqual((plan["version"], registry["version"]), (6, 6))
        self.assertEqual(plan["limits"]["maxSteps"], 3)
        self.assertEqual(plan["limits"]["maxReasonerCalls"], 1)
        expected_chain = (("open_page", "open-page", "read_page"),
                          ("read_page", "read-page", "summarize"),
                          ("summarize", "summarize-page", "done"))
        for node, task, following in expected_chain:
            self.assertEqual(plan["nodes"][node], {"kind": "task", "taskRef": task, "onVerified": following})
        for task in tasks.values():
            self.assertIn(task["prompt"], files)
        self.assertEqual([tasks[name]["loop"]["maxSteps"] for name in
                          ("open-page", "read-page", "summarize-page")], [1, 1, 1])
        for name, grant in (("open-page", "e2llm.act"),
                            ("read-page", "e2llm.read_page")):
            self.assertEqual(tasks[name]["tools"], [grant])
            self.assertEqual(tasks[name]["completion"], "tool-result")
            prompt = files[tasks[name]["prompt"]].decode()
            self.assertNotIn("{{ user_input }}", prompt)
            self.assertIn(f'"server":"e2llm","tool":"{grant.split(".")[1]}","args":{{}}', prompt)
            for obsolete in ('"BROWSER"', '"URL"', '"tabId":123', "<tool_result>", "acknowledg"):
                self.assertNotIn(obsolete, prompt)
        self.assertEqual(tasks["open-page"]["outputContract"], {
            "outputs": {"opened": {"fromArtifact": "opened", "format": "json", "type": "object"}},
            "requireArtifactComplete": ["opened"],
        })
        self.assertNotIn("allowNoop", tasks["open-page"]["outputContract"])
        self.assertEqual(tasks["open-page"]["mutations"], {"idempotency": "intent-log", "onUncertain": "fail-run"})
        self.assertEqual(tasks["summarize-page"]["tools"], [])
        self.assertEqual(tasks["summarize-page"]["model"], "reasoner")
        self.assertIn("{{ read_page.outputs.page }}", files["prompts/summarize-page.md"].decode())
        self.assertNotIn("outputs.report", files["prompts/summarize-page.md"].decode())
        self.assertNotIn("list_tabs.outputs.tabs", files["prompts/summarize-page.md"].decode())
        self.assertNotIn("list_tabs", files["prompts/read-page.md"].decode())
        self.assertIn("Omit tabId", files["prompts/read-page.md"].decode())
        self.assertEqual(tasks["open-page"]["toolArguments"], {
            "e2llm.act": {"fromInput": {"browser": "browser", "target": "url"}, "fixed": {"action": "open"}},
        })
        self.assertEqual(tasks["read-page"]["toolArguments"], {
            "e2llm.read_page": {"fromInput": {"browser": "browser"},
                                "fromVars": {"tabId": "open_page.outputs.opened.tabId"}},
        })
        self.assertEqual(tasks["read-page"]["outputContract"], {
            "outputs": {"page": {"fromArtifact": "page", "format": "json", "type": "object"}},
            "requireArtifactComplete": ["page"],
        })
        policies = plan["mcp"]["policies"]
        self.assertNotIn("e2llm.list_tabs", policies)
        self.assertEqual(policies["e2llm.act"]["resultArtifact"], "opened")
        opened_validator = Draft202012Validator(policies["e2llm.act"]["resultValidation"]["schema"])
        opened = {"result": "opened", "tabId": 178, "url": "https://example.org/"}
        opened_validator.validate(opened)
        for invalid in ({**opened, "result": "unknown"}, {**opened, "tabId": 0},
                        {**opened, "tabId": "178"}, {**opened, "url": ""},
                        {key: value for key, value in opened.items() if key != "tabId"}):
            self.assertFalse(opened_validator.is_valid(invalid))
        self.assertTrue(all("retry" not in p and "cache" not in p for p in policies.values()))
        self.assertEqual(policies["e2llm.read_page"]["shape"], {"strip": ["content"], "toFile": True})
        validator = Draft202012Validator(policies["e2llm.read_page"]["resultValidation"]["schema"])
        page = {"content": "Exact page", "url": "https://example.org/", "stats": {"contentNodes": 1, "truncated": False}}
        validator.validate(page)
        for invalid in ({**page, "content": ""}, {**page, "selectorUnmatched": True},
                        {**page, "stats": {"contentNodes": 1, "truncated": True}}):
            self.assertFalse(validator.is_valid(invalid))

    def test_check_total_manifest_setup_and_local_bindings(self):
        data = fixture.build_bundle(starter="check-total")
        self.assertEqual(data, fixture.CHECK_TOTAL_ASSET.read_bytes())
        self.assertEqual(data, fixture.build_bundle(starter="check-total"))
        self.assertLess(len(data), 32 * 1024)
        files = self.unpack("check-total")
        manifest = json.loads(files.pop("manifest.json"))
        self.assertEqual(set(files), set(fixture.CHECK_TOTAL_FILES))
        self.assertEqual({entry["path"] for entry in manifest["files"]}, set(files))
        for entry in manifest["files"]:
            self.assertEqual(entry["sha256"], hashlib.sha256(files[entry["path"]]).hexdigest())
        plan = yaml.safe_load(files[manifest["entryFlow"]])
        setup = json.loads(files["setup.json"])
        registry = yaml.safe_load(files[manifest["taskRegistry"]])
        for schema, value in (("execution-plan.v1.schema.json", plan),
                              ("flow-setup.v1.schema.json", setup)):
            Draft202012Validator(json.loads((fixture.REPO / "schemas" / schema).read_text())).validate(value)
        self.assertEqual((plan["id"], registry["id"], plan["version"], registry["version"]),
                         ("check-total", "check-total", 1, 1))
        self.assertEqual(plan["setup"]["sha256"], hashlib.sha256(files["setup.json"]).hexdigest())
        self.assertEqual(setup["title"], "Check a total")
        self.assertEqual(setup["connections"], {})
        self.assertEqual(plan["mcp"]["servers"], [])
        browser_setup = json.loads((fixture.BROWSER_SOURCE / "setup.json").read_text())
        self.assertEqual(setup["models"], browser_setup["models"])
        self.assertEqual(plan["models"], {slot: {"ctx": model["ctx"]}
                                          for slot, model in setup["models"].items()})
        self.assertEqual([(field["key"], field["kind"], field["required"], field["maxLength"])
                          for field in setup["input"]["fields"]],
                         [("text", "text", True, 2048), ("budget", "text", True, 20)])
        self.assertNotIn("toolArguments", setup["input"])
        tasks = registry["tasks"]
        self.assertEqual(tasks["sum-cart"]["toolArguments"],
                         {"local.calculate": {"fixed": {"operation": "add"}}})
        self.assertEqual(tasks["compare-budget"]["toolArguments"], {"local.calculate": {
            "fromVars": {"left": "sum.outputs.calculation.result"},
            "fromInput": {"right": "budget"}, "fixed": {"operation": "compare"}}})
        for name in ("sum-cart", "compare-budget"):
            self.assertEqual(tasks[name]["tools"], ["local.calculate"])
            self.assertEqual(tasks[name]["completion"], "tool-result")
            self.assertEqual(tasks[name]["outputContract"]["requireArtifactComplete"], ["calculation"])
            self.assertIn('<tool_call>{"server":"local","tool":"calculate","args":',
                          files[tasks[name]["prompt"]].decode())
        for name, slot in (("advise-cart", "reasoner"), ("report-cart", "coder")):
            self.assertEqual(tasks[name]["tools"], [])
            self.assertEqual(tasks[name]["model"], slot)
            prompt = files[tasks[name]["prompt"]].decode()
            for binding in ("user_input", "sum.outputs.calculation", "compare.outputs.calculation"):
                self.assertIn("{{ " + binding + " }}", prompt)
        self.assertIn("{{ advise.outputs.advice }}", files[tasks["report-cart"]["prompt"]].decode())
        policy = plan["mcp"]["policies"]["local.calculate"]
        self.assertEqual((policy["class"], policy["resultArtifact"]), ("read", "calculation"))

    def test_all_assets_are_installed_and_explicit_output_retains_text_default(self):
        source = (fixture.REPO / "apps/android/app/src/main/java/com/trollm/app/StarterFlows.kt").read_text()
        for name in fixture.STARTERS:
            self.assertIn(f'"flows/{name}.tar"', source)
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            path = Path(directory) / "starter.tar"
            fixture.main(["--output", str(path)])
            self.assertEqual(path.read_bytes(), fixture.build_bundle())


if __name__ == "__main__":
    unittest.main()
