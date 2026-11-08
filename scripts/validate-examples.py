#!/usr/bin/env python3
"""Check plan schemas/edges and complete reference registries and prompts.

Procurement is a design skeleton: its registry and evaluate-product subflow are
not supplied. Validate its schema/edges, but do not present it as executable.
"""
import json
from pathlib import Path

import yaml
from jsonschema import Draft202012Validator
from flow_checks import validate_fixed_tool_arguments

ROOT = Path(__file__).resolve().parents[1]
schema = json.loads((ROOT / "schemas/execution-plan.v1.schema.json").read_text())
Draft202012Validator.check_schema(schema)
validator = Draft202012Validator(schema)
task_result_schema = json.loads((ROOT / "schemas/task-result-artifact.v1.schema.json").read_text())
Draft202012Validator.check_schema(task_result_schema)
task_result_validator = Draft202012Validator(task_result_schema)
plans = {}
starter_plans = sorted((ROOT / "flows/starters").glob("*/*.flow.yaml"))
for path in sorted((ROOT / "flows/examples").glob("*.flow.yaml")) + starter_plans:
    plan = yaml.safe_load(path.read_text())
    errors = sorted(validator.iter_errors(plan), key=lambda error: str(error.path))
    if errors:
        raise ValueError(f"{path}: " + "; ".join(error.message for error in errors))
    if plan["id"] in plans:
        raise ValueError(f"duplicate flow ID: {plan['id']}")
    plans[plan["id"]] = plan
    nodes = plan["nodes"]
    if plan["entryStep"] not in nodes:
        raise ValueError(f"{path}: missing entry step")
    for node in nodes.values():
        edges = [node.get(key) for key in ("onVerified", "onEmpty", "onComplete", "onLimit")]
        edges.extend(node.get("onItemOutcome", {}).values())
        edges.append(node.get("onError", {}).get("then"))
        edges.extend(node.get("routes", {}).values())
        for route in node.get("routes", {}).values():
            if isinstance(route, dict) and route["status"] == "paused" and "resumeAt" not in route:
                raise ValueError(f"{path}: paused route requires resumeAt")
        for edge in edges:
            if isinstance(edge, dict) and "resumeAt" in edge:
                if edge["status"] != "paused" or edge["resumeAt"] not in nodes:
                    raise ValueError(f"{path}: invalid resumeAt continuation")
            if isinstance(edge, str) and edge not in nodes:
                raise ValueError(f"{path}: unknown transition {edge}")
    print(f"schema and transitions OK: {path.relative_to(ROOT)}")

references = [(name, ROOT / "flows/tasks" / registry) for name, registry in (
    ("ticket-triage", "tickets.tasks.yaml"),
    ("minimal-summary", "minimal-summary.tasks.yaml"),
    ("prose-calculation", "prose-calculation.tasks.yaml"),
    ("adaptive-routing", "adaptive-routing.tasks.yaml"),
)]
# A starter package can contain reusable child plans sharing its root registry.
# Validate every plan above, then resolve each package from its named root.
references.extend((directory.name,
                   directory / f"{directory.name}.tasks.yaml")
                  for directory in sorted((ROOT / "flows/starters").iterdir())
                  if directory.is_dir() and (directory / f"{directory.name}.flow.yaml").is_file())
for root_flow, registry_path in references:
    registry = yaml.safe_load(registry_path.read_text())
    assert registry["schemaVersion"] == "ledger.task-registry.v1"
    for name, task in registry["tasks"].items():
        task_result_validator.validate(task)
        validate_fixed_tool_arguments(task, ROOT / "schemas")
        for output in task.get("outputContract", {}).get("outputs", {}).values():
            if "schema" in output:
                if not ("fromArtifact" in output or output.get("from") == "model") or not isinstance(output["schema"], dict):
                    raise ValueError(f"{name}: schema requires a model/artifact output and JSON object")
                Draft202012Validator.check_schema(output["schema"])
            if "view" in output:
                binding = {key: value for key, value in output.items() if key not in ("type", "schema")}
                Draft202012Validator({"$defs": schema["$defs"], "$ref": "#/$defs/nodeOutputBinding"}).validate(binding)
        prompt = (registry_path.parent / task["prompt"]).resolve()
        if not prompt.is_relative_to(registry_path.parent.resolve()) or not prompt.is_file():
            raise ValueError(f"{name}: missing or escaping prompt")
        if task["model"] not in ("coder", "reasoner"):
            raise ValueError(f"{name}: unknown model")

    pending, seen = [root_flow], set()
    while pending:
        flow_id = pending.pop()
        if flow_id in seen:
            continue
        seen.add(flow_id)
        if flow_id not in plans:
            raise ValueError(f"missing subflow: {flow_id}")
        for node in plans[flow_id]["nodes"].values():
            if node["kind"] == "task" and node["taskRef"] not in registry["tasks"]:
                raise ValueError(f"{flow_id}: missing task {node['taskRef']}")
            if node["kind"] == "task":
                task = registry["tasks"][node["taskRef"]]
                completion = task.get("completion", "model-prose")
                routing = completion in ("flow-route", "tool-or-route")
                if routing != ("routes" in node) or (("onVerified" in node) != (completion != "flow-route")):
                    raise ValueError(f"{flow_id}: routes require flow-route completion")
                if routing and (task["model"] != "coder" or "flow.choose" not in task.get("tools", [])):
                    raise ValueError(f"{flow_id}: routing requires coder and flow.choose")
                if len(node.get("promptSupplement", "").encode("utf-8")) > 16 * 1024:
                    raise ValueError(f"{flow_id}: promptSupplement exceeds 16 KiB")
            for key in ("body", "between"):
                if key in node:
                    pending.append(node[key]["flowRef"])
    print(f"{root_flow} references and prompts OK ({len(seen)} flows)")
print("procurement: schema-only design skeleton (not an executable bundle)")

# Optional business descriptions are checked against their actual source package.
# They never become part of execution-plan YAML or runtime control flow.
presentation_schema = json.loads(
    (ROOT / "apps/flow-review/flow-review.schema.json").read_text()
)
Draft202012Validator.check_schema(presentation_schema)
presentation_validator = Draft202012Validator(presentation_schema)
for path in sorted((ROOT / "flows/starters").glob("*/flow-review.json")):
    presentation = json.loads(path.read_text())
    presentation_validator.validate(presentation)
    package_plans = [yaml.safe_load(p.read_text()) for p in path.parent.rglob("*.flow.yaml")]
    actual_nodes = {(p["id"], node) for p in package_plans for node in p["nodes"]}
    phase_ids, assigned = set(), set()
    for phase in presentation["phases"]:
        if phase["id"] in phase_ids:
            raise ValueError(f"{path}: duplicate presentation phase {phase['id']}")
        phase_ids.add(phase["id"])
        for ref in phase["nodes"]:
            key = (ref["flow"], ref["node"])
            if key not in actual_nodes or key in assigned:
                raise ValueError(f"{path}: unknown or duplicate phase reference {key}")
            assigned.add(key)
    print(f"presentation schema and phase references OK: {path.relative_to(ROOT)}")
