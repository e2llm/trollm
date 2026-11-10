import hashlib
import io
import json
from pathlib import Path
import tarfile
import unittest

from jsonschema import Draft202012Validator
import make_selection_replay as replay


class SelectionReplayTests(unittest.TestCase):
    # Synthetic transfer records; these are not claimed browser captures.
    records = [{"id": "search", "tag": "input", "aria": {"label": "Search catalog"}},
               {"id": "submit", "tag": "button", "text": "Go"}]

    def files(self, **kwargs):
        data = replay.build_bundle(self.records, "Find the catalog search field", **kwargs)
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            return {member.name: archive.extractfile(member).read() for member in archive}

    def test_bundle_is_deterministic_pinned_and_read_only(self):
        for advice in [None, "The search input is appropriate.", "No password field is shown."]:
            with self.subTest(advice=advice):
                files = self.files(advice=advice)
                self.assertEqual(files, self.files(advice=advice))
                manifest = json.loads(files["manifest.json"])
                for entry in manifest["files"]:
                    self.assertEqual(entry["sha256"], hashlib.sha256(files[entry["path"]]).hexdigest())
                plan = json.loads(files[manifest["entryFlow"]])
                schema_path = Path(__file__).resolve().parents[2] / "schemas/execution-plan.v1.schema.json"
                Draft202012Validator(json.loads(schema_path.read_text())).validate(plan)
                self.assertNotIn("servers", plan["mcp"])
                registry = json.loads(files[manifest["taskRegistry"]])
                tools = {tool for task in registry["tasks"].values() for tool in task["tools"]}
                self.assertEqual(tools, {"local.filter_records", "local.prepare_worklist", "local.select_unique", "flow.choose"})
                select = registry["tasks"]["select"]
                self.assertEqual(select["toolArguments"]["local.select_unique"]["modelFields"], ["equals"])
                self.assertEqual(plan["nodes"]["select"]["routes"], {"no_selection": "declined"})
                self.assertEqual("reasoner" in plan["models"], advice is None)

    def test_record_text_and_advice_braces_are_data_not_template_instructions(self):
        records = [{"id": "sample", "text": "{{ secret }}"}]
        data = replay.build_bundle(records, "{{ question }}", advice="{{ advice }}")
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            prompts = b"".join(archive.extractfile(m).read() for m in archive if m.name.endswith(".md"))
        for data in [b"{{ secret }}", b"{{ question }}", b"{{ advice }}"]:
            self.assertNotIn(data, prompts)

    def test_invalid_or_oversized_sources_are_refused_without_slicing(self):
        for records in [[], [{}], [{"id": 1}], [None], [{"id": "x", "text": "a" * (1024 * 1024)}]]:
            with self.assertRaises(ValueError):
                replay.build_bundle(records, "Find an entry")
        with self.assertRaises(ValueError):
            replay.build_bundle(self.records, "a" * 4097)

    def test_text_lookup_binds_original_fields_and_leaves_only_the_reference_model_owned(self):
        files = self.files(match_fields=["/aria/label", "/text"], advice="Use the search label.")
        registry = json.loads(files["selection-replay.tasks.yaml"])
        binding = registry["tasks"]["select"]["toolArguments"]["local.select_unique"]
        self.assertEqual(binding["modelFields"], ["equals"])
        self.assertEqual(binding["fixed"], {
            "field": ["/selected/aria/label", "/selected/text"],
            "matchMode": "string-contains", "prefix": ""})
        original = registry["tasks"]["observe"]["toolArguments"]["local.filter_records"]["fixed"]["items"]
        self.assertEqual(original, self.records)
        schema = json.loads((Path(__file__).resolve().parents[2] /
                            "schemas/local-select-unique.v1.input.schema.json").read_text())
        Draft202012Validator(schema).validate({**binding["fixed"], "items": [{"selected": r} for r in original], "equals": "Search catalog"})
        self.assertIn(b"literal text observed", files["prompts/select.md"])
        self.assertNotIn("stopBefore", binding["fixed"])

    def test_text_lookup_allows_original_records_without_synthetic_ids(self):
        replay.build_bundle([{"title": "An original ticket", "number": 123}],
                            "Find this ticket", match_fields=["/title"])
        for fields in ([], ["title"], ["/bad~2"], [7], ["/text"] * 33, "title"):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                self.files(match_fields=fields)
        with self.assertRaises(ValueError):
            self.files(stop_before=",")
        files = self.files(match_fields=["/text"], stop_before=",")
        registry = json.loads(files["selection-replay.tasks.yaml"])
        self.assertEqual(registry["tasks"]["select"]["toolArguments"]["local.select_unique"]["fixed"]["stopBefore"], ",")


if __name__ == "__main__":
    unittest.main()
