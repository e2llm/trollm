from pathlib import Path
import json
import unittest

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError
from flow_checks import validate_fixed_tool_arguments


class NodeIdentifierTests(unittest.TestCase):
    def test_schema_rejects_path_names_in_keys_and_references(self):
        schema = json.loads((Path(__file__).resolve().parents[1] /
                             "schemas/execution-plan.v1.schema.json").read_text())
        Draft202012Validator.check_schema(schema)
        fragments = [schema["properties"]["nodes"], schema["properties"]["entryStep"],
                     {"$ref": "#/$defs/transition"}]
        bad = ["", ".", "..", "./../escaped", "a/b", "a\\b", "C:drive", "a.b",
               "space name", "line\n", "é", "n" * 129]
        for name in bad + ["n", "9_A-z", "n" * 128]:
            expected = name not in bad
            for fragment in fragments:
                validator = Draft202012Validator({"$defs": schema["$defs"], **fragment})
                value = ({name: {"kind": "end", "outcome": "succeeded"}}
                         if fragment is fragments[0] else name)
                with self.subTest(name=name, fragment=fragment):
                    self.assertEqual(validator.is_valid(value), expected)
            transition = Draft202012Validator({"$defs": schema["$defs"], "$ref": "#/$defs/transition"})
            self.assertEqual(transition.is_valid({"status": "paused", "resumeAt": name}), expected)


class FixedArgumentTests(unittest.TestCase):
    def test_operation_names_are_data_and_not_node_references(self):
        schemas = Path(__file__).resolve().parents[1] / "schemas"
        task = {"toolArguments": {"local.calculate": {"fixed": {"operation": "add"},
                "fromVars": {"left": "original.amount", "right": "original.other"}}}}
        validate_fixed_tool_arguments(task, schemas)
        task["toolArguments"]["local.calculate"]["fixed"]["operation"] = "repair_add"
        with self.assertRaises(ValidationError):
            validate_fixed_tool_arguments(task, schemas)

    def test_unknown_tool_and_unknown_fixed_argument_are_rejected(self):
        schemas = Path(__file__).resolve().parents[1] / "schemas"
        with self.assertRaises(ValueError):
            validate_fixed_tool_arguments({"toolArguments": {"local.invented": {}}}, schemas)
        with self.assertRaises(ValidationError):
            validate_fixed_tool_arguments({"toolArguments": {"local.calculate": {"fixed": {"operator": "add"}}}}, schemas)


    def test_unique_selection_conditional_schema_allows_a_bound_field(self):
        schemas = Path(__file__).resolve().parents[1] / "schemas"
        validate_fixed_tool_arguments({"toolArguments": {"local.select_unique": {
            "fixed": {"prefix": ""}, "fromVars": {"field": "tier.field", "items": "original.items"}, "modelFields": ["equals"]}}}, schemas)
        validate_fixed_tool_arguments({"toolArguments": {"local.select_unique": {
            "fixed": {"prefix": "", "field": ["/text", "/aria/label"], "matchMode": "string-contains", "stopBefore": ","}, "modelFields": []}}}, schemas)
        with self.assertRaises(ValidationError):
            validate_fixed_tool_arguments({"toolArguments": {"local.select_unique": {
                "fixed": {"prefix": "", "field": ["/text"], "matchMode": "equals"}, "modelFields": []}}}, schemas)


if __name__ == "__main__":
    unittest.main()
