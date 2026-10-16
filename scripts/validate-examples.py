#!/usr/bin/env python3
"""Check plan schemas/edges and the ticket-triage and minimal-summary references.

Procurement is a design skeleton: its registry and evaluate-product subflow are
not supplied. Validate its schema/edges, but do not present it as executable.
"""
import json
from pathlib import Path

import yaml
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
schema = json.loads((ROOT / "schemas/execution-plan.v1.schema.json").read_text())
Draft202012Validator.check_schema(schema)
validator = Draft202012Validator(schema)
plans = {}
for path in sorted((ROOT / "flows/examples").glob("*.flow.yaml")):
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
        edges.append(node.get("onError", {}).get("then"))
        for edge in edges:
            if isinstance(edge, str) and edge not in nodes:
                raise ValueError(f"{path}: unknown transition {edge}")
    print(f"schema and transitions OK: {path.relative_to(ROOT)}")

for root_flow, registry_file in (
    ("ticket-triage", "tickets.tasks.yaml"),
    ("minimal-summary", "minimal-summary.tasks.yaml"),
):
    registry_path = ROOT / "flows/tasks" / registry_file
    registry = yaml.safe_load(registry_path.read_text())
    assert registry["schemaVersion"] == "ledger.task-registry.v1"
    for name, task in registry["tasks"].items():
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
            for key in ("body", "between"):
                if key in node:
                    pending.append(node[key]["flowRef"])
    print(f"{root_flow} references and prompts OK ({len(seen)} flows)")
print("procurement: schema-only design skeleton (not an executable bundle)")
