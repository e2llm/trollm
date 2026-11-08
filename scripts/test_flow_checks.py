from pathlib import Path
import unittest

from jsonschema.exceptions import ValidationError
from flow_checks import validate_fixed_tool_arguments


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


if __name__ == "__main__":
    unittest.main()
