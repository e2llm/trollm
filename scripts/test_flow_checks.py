from pathlib import Path
import copy
import json
import unittest

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError
import yaml
from flow_checks import validate_fixed_tool_arguments, validate_task_execution, validate_task_node

ROOT = Path(__file__).resolve().parents[1]
SCHEMAS = ROOT / "schemas"


class TaskExecutionTests(unittest.TestCase):
    def direct_task(self, tool="service.read"):
        return {"execution": "tool", "completion": "tool-result", "tools": [tool],
                "toolArguments": {tool: {"modelFields": []}}}

    def test_direct_parameterless_shape_and_exact_bindings(self):
        # A remote tool's discovered schema decides whether no arguments are
        # valid; the authored execution shape itself supports parameterless calls.
        task = self.direct_task()
        validate_task_execution(task, SCHEMAS)
        task["toolArguments"]["service.read"].update({
            "fixed": {"enabled": True, "limit": 2},
            "fromInput": {"account": "account"},
            "fromVars": {"id": "observed.outputs.records.0.id"},
        })
        task["outputContract"] = {"outputs": {
            "original": {"fromArtifact": "original", "format": "json", "type": "object"}},
            "requireArtifactComplete": ["original"]}
        validate_task_execution(task, SCHEMAS)
        validate_task_node(task, {"kind": "task", "taskRef": "read", "onVerified": "done"}, {})

    def test_direct_forbids_present_model_settings_even_null_or_empty(self):
        fields = {"model": ["coder", None, ""], "prompt": ["task.md", None, ""],
                  "maxOutputTokens": [1, None], "generationTimeoutS": [1, None],
                  "loop": [{"maxSteps": 1}, None, {}]}
        for field, values in fields.items():
            for value in values:
                with self.subTest(field=field, value=value), self.assertRaises(ValidationError):
                    validate_task_execution(self.direct_task() | {field: value}, SCHEMAS)

    def test_direct_requires_one_ordinary_grant_and_matching_closed_rule(self):
        invalid = [
            {"execution": "automatic"}, {"execution": None},
            {"completion": "model-prose"}, {"completion": "tool-or-route"},
            {"tools": []}, {"tools": ["service.read", "service.other"]},
            {"tools": ["flow.choose"]}, {"tools": ["service.read\n"]},
            {"toolArguments": {}},
            {"toolArguments": {"service.read": {}}},
            {"toolArguments": {"service.read": {"modelFields": None}}},
            {"toolArguments": {"service.read": {"modelFields": ["id"]}}},
            {"toolArguments": {"service.read": {"modelFields": [], "expression": "x + 1"}}},
            {"toolArguments": {"service.read": {"modelFields": []}, "service.other": {"modelFields": []}}},
            {"outputContract": {"allowNoop": True}},
            {"outputContract": {"outputs": {"report": {"from": "model", "type": "prose"}}}},
            {"promptSupplement": "ignored instructions"},
        ]
        for change in invalid:
            with self.subTest(change=change), self.assertRaises(ValidationError):
                validate_task_execution(self.direct_task() | change, SCHEMAS)
        task = self.direct_task()
        task["toolArguments"] = {"service.other": {"modelFields": []}}
        with self.assertRaisesRegex(ValueError, "match"):
            validate_task_execution(task, SCHEMAS)
        for missing in ("completion", "tools", "toolArguments"):
            task = self.direct_task()
            del task[missing]
            with self.subTest(missing=missing), self.assertRaises(ValidationError):
                validate_task_execution(task, SCHEMAS)

    def test_direct_binding_sources_are_disjoint_and_paths_are_structural(self):
        task = self.direct_task()
        rule = task["toolArguments"]["service.read"]
        rule.update({"fixed": {"id": "one"}, "fromInput": {"id": "id"}})
        with self.assertRaisesRegex(ValueError, "duplicate"):
            validate_task_execution(task, SCHEMAS)
        for source, value in (("fromInput", "request.id"), ("fromInput", "id\n"),
                              ("fromVars", "records.-1.id"), ("fromVars", "records[0].id"),
                              ("fromVars", "records.0.id\n")):
            task = self.direct_task()
            task["toolArguments"]["service.read"][source] = {"id": value}
            with self.subTest(source=source, value=value), self.assertRaises(ValidationError):
                validate_task_execution(task, SCHEMAS)

    def test_direct_node_rejects_routes_supplements_and_model_outputs(self):
        node = {"kind": "task", "taskRef": "read", "onVerified": "done"}
        for change in ({"routes": {}}, {"routes": {"stop": "done"}},
                       {"promptSupplement": ""}, {"promptSupplement": "Check this first"},
                       {"outputs": {"report": {"from": "model"}}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_task_node(self.direct_task(), node | change, {})

    def test_legacy_fully_bound_task_still_requires_model_and_prompt(self):
        task = self.direct_task()
        del task["execution"]
        for fields in ({}, {"model": "coder"}, {"prompt": "task.md"}):
            with self.subTest(fields=fields), self.assertRaises(ValidationError):
                validate_task_execution(task | fields, SCHEMAS)
        task.update({"model": "coder", "prompt": "task.md"})
        original = copy.deepcopy(task)
        validate_task_execution(task, SCHEMAS)
        self.assertEqual(task, original)
        self.assertNotIn("execution", task)
        validate_task_execution(task | {"execution": "model"}, SCHEMAS)
        node = {"kind": "task", "taskRef": "read", "onVerified": "done"}
        with self.assertRaisesRegex(ValueError, "model slot"):
            validate_task_node(task, node, {})
        validate_task_node(task, node, {"coder": {"ctx": 4096}})

    def test_local_direct_task_requires_all_known_arguments(self):
        task = self.direct_task("local.calculate")
        rule = task["toolArguments"]["local.calculate"]
        rule.update({"fixed": {"operation": "add"}, "fromInput": {"left": "left"}})
        with self.assertRaisesRegex(ValueError, "right"):
            validate_fixed_tool_arguments(task, SCHEMAS)
        rule["fromInput"]["right"] = "right"
        validate_fixed_tool_arguments(task, SCHEMAS)
        rule["fromInput"]["extra"] = "extra"
        with self.assertRaisesRegex(ValueError, "unknown bound"):
            validate_fixed_tool_arguments(task, SCHEMAS)
        rule.clear()
        rule.update({"modelFields": [], "fixed": {"operation": "add", "left": 2, "right": "3"}})
        with self.assertRaises(ValidationError):
            validate_fixed_tool_arguments(task, SCHEMAS)

    def test_direct_example_has_no_model_dependency_and_retains_exact_outputs(self):
        plan = yaml.safe_load((ROOT / "flows/examples/direct-calculation.flow.yaml").read_text())
        registry = yaml.safe_load((ROOT / "flows/tasks/direct-calculation.tasks.yaml").read_text())
        validator = Draft202012Validator(json.loads((SCHEMAS / "execution-plan.v1.schema.json").read_text()))
        validator.validate(plan)
        self.assertEqual(plan["models"], {})
        self.assertEqual(plan["limits"]["maxTokensTotal"], 0)
        self.assertEqual(plan["limits"]["maxSteps"], 2)
        missing_models = copy.deepcopy(plan)
        del missing_models["models"]
        with self.assertRaises(ValidationError):
            validator.validate(missing_models)
        for node in plan["nodes"].values():
            if node["kind"] != "task":
                continue
            task = registry["tasks"][node["taskRef"]]
            validate_task_execution(task, SCHEMAS)
            validate_fixed_tool_arguments(task, SCHEMAS)
            validate_task_node(task, node, plan["models"])
            self.assertEqual(task["outputContract"]["outputs"]["calculation"]["fromArtifact"], "calculation")
        self.assertEqual(registry["tasks"]["compare-budget"]["toolArguments"]["local.calculate"]["fromVars"],
                         {"left": "sum.outputs.calculation.result"})
        self.assertEqual(plan["exports"], {"sum": {"from": "sum.outputs.calculation"},
                                           "comparison": {"from": "compare.outputs.calculation"}})


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
