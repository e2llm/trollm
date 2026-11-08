"""Portable checks for authored constants; dynamic bindings remain runtime checked."""
import json
from pathlib import Path

from jsonschema import Draft202012Validator


def validate_fixed_tool_arguments(task, schemas):
    for tool, arguments in task.get("toolArguments", {}).items():
        if not tool.startswith("local."):
            continue
        path = Path(schemas) / (tool.replace(".", "-", 1).replace("_", "-") + ".v1.input.schema.json")
        if not path.is_file():
            raise ValueError(f"Unknown built-in tool contract: {tool}")
        schema = json.loads(path.read_text())
        # Bindings fill other top-level arguments later. Constants still must
        # have correct types, names, enums and nested required fields now.
        schema.pop("required", None)
        fixed = arguments.get("fixed", {})
        Draft202012Validator(schema).validate(fixed)
        if "schema" in fixed:
            Draft202012Validator.check_schema(fixed["schema"])
