"""Portable task/graph checks; dynamic binding values remain runtime checked."""
from functools import lru_cache
import json
from pathlib import Path

from jsonschema import Draft202012Validator


@lru_cache(maxsize=None)
def _execution_validator(schemas):
    schema = json.loads((Path(schemas) / "task-execution.v1.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


def validate_task_execution(task, schemas):
    """Check explicit execution mode without inferring intent from arguments."""
    _execution_validator(str(schemas)).validate(task)
    if task.get("execution", "model") != "tool":
        return
    tool = task["tools"][0]
    if set(task["toolArguments"]) != {tool}:
        raise ValueError("direct toolArguments must match the single granted tool")
    rule = task["toolArguments"][tool]
    bound = set()
    for source in ("fixed", "fromInput", "fromVars"):
        names = set(rule.get(source, {}))
        if bound & names:
            raise ValueError("direct toolArguments has duplicate argument sources")
        bound.update(names)


def validate_task_node(task, node, models):
    """Check the contracts that need both the plan node and its registry task."""
    direct = task.get("execution", "model") == "tool"
    if direct:
        if "routes" in node or "promptSupplement" in node:
            raise ValueError("direct tool tasks cannot declare routes or promptSupplement")
        if any(output.get("from") == "model" for output in node.get("outputs", {}).values()):
            raise ValueError("direct tool tasks cannot bind model outputs")
    elif task["model"] not in models:
        raise ValueError(f"model slot {task['model']!r} is not declared in plan.models")

    completion = task.get("completion", "model-prose")
    routing = completion in ("flow-route", "tool-or-route")
    if routing != ("routes" in node) or (("onVerified" in node) != (completion != "flow-route")):
        raise ValueError("flow-route requires routes without onVerified; tool-or-route requires "
                         "both; other completions require onVerified without routes")
    if routing and (task.get("model") != "coder" or "flow.choose" not in task.get("tools", [])):
        raise ValueError("routing requires coder and flow.choose")
    if len(node.get("promptSupplement", "").encode("utf-8")) > 16 * 1024:
        raise ValueError("promptSupplement exceeds 16 KiB")


def validate_fixed_tool_arguments(task, schemas):
    for tool, arguments in task.get("toolArguments", {}).items():
        if not tool.startswith("local."):
            continue
        path = Path(schemas) / (tool.replace(".", "-", 1).replace("_", "-") + ".v1.input.schema.json")
        if not path.is_file():
            raise ValueError(f"Unknown built-in tool contract: {tool}")
        schema = json.loads(path.read_text())
        fixed = arguments.get("fixed", {})
        if task.get("execution", "model") == "tool":
            bound = set(fixed) | set(arguments.get("fromVars", {})) | set(arguments.get("fromInput", {}))
            missing = set(schema.get("required", [])) - bound
            if missing:
                raise ValueError(f"{tool}: required arguments are not bound: {', '.join(sorted(missing))}")
            if schema.get("additionalProperties") is False:
                unknown = bound - set(schema.get("properties", {}))
                if unknown:
                    raise ValueError(f"{tool}: unknown bound arguments: {', '.join(sorted(unknown))}")
            if not arguments.get("fromVars") and not arguments.get("fromInput"):
                Draft202012Validator(schema).validate(fixed)
        # Bindings fill other top-level arguments later. Constants still must
        # have correct types, names, enums and nested required fields now.
        schema.pop("required", None)
        Draft202012Validator(schema).validate(fixed)
        if "schema" in fixed:
            Draft202012Validator.check_schema(fixed["schema"])
