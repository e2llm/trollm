#!/usr/bin/env python3
"""Generate tiny TEST-ONLY bundles for a stub Android build, without downloads.

Run from any directory. All artifacts land in .cache/android-smoke. The valid
bundle runs one tool-free task and succeeds with ``stub model response`` when
the APK uses the deterministic native stub backend. It does not test inference.
The bad-hash bundle has identical payload bytes but an incorrect prompt digest,
so the file-picker install must reject it before publishing an installed flow.
The pause bundle declares a sentinel tool with no server: the stub never calls
it, so verification pauses with needs-attention. Resume repeats that task;
cancel ends with reason cancelled. No bundle executes tools or accesses a server.
JSON documents use .yaml names because JSON is a subset of YAML; no PyYAML needed.
"""

import hashlib
import io
import json
from pathlib import Path
import tarfile

FLOW_ID = "android-smoke"
PAUSED_FLOW_ID = "android-smoke-paused"
PAUSE_REASON = "needs-attention"
FLOW_FILE = f"{FLOW_ID}.flow.yaml"
REGISTRY_FILE = f"{FLOW_ID}.tasks.yaml"
PROMPT_FILE = "prompts/smoke.md"
EXPECTED_REPORT = "stub model response"
USER_INPUT = "android smoke test"
OUTPUT_DIR = Path(__file__).resolve().parents[2] / ".cache" / "android-smoke"


def json_bytes(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def fixture_files(*, paused=False):
    """The bounded no-network fixture mirrors the native stub lifecycle test."""
    plan = {
        "schemaVersion": "ledger.execution-plan.v1",
        "id": FLOW_ID,
        "version": 1,
        "sourceBaseline": {"sha256": "0" * 64},
        "input": {"promptTemplate": "TEST ONLY: {{ user_input }}"},
        "limits": {"maxSteps": 4, "maxReasonerCalls": 0,
                   "maxTokensTotal": 1000, "timeoutS": 30, "artifactMaxMb": 1},
        "onLimit": "abort",
        "models": {"coder": {"ctx": 2048}},
        "mcp": {"servers": [], "policies": {}},
        "entryStep": "report",
        "nodes": {
            "report": {"kind": "task", "taskRef": "smoke-report", "onVerified": "done"},
            "done": {"kind": "end", "outcome": "succeeded"},
        },
    }
    registry = {
        "schemaVersion": "ledger.task-registry.v1",
        "id": FLOW_ID,
        "version": 1,
        "tasks": {"smoke-report": {
            "model": "coder", "prompt": PROMPT_FILE, "tools": [],
            "loop": {"maxSteps": 1},
            "outputContract": {"outputs": {"report": {"from": "model", "type": "prose"}}},
        }},
    }
    if paused:
        # Same failure contract as integration.rs PAUSING_FLOW/REGISTRY:
        # stub returns prose without calling the declared sentinel tool, and
        # allowNoop defaults false. No server exists and no tool is executed.
        plan["id"] = PAUSED_FLOW_ID
        plan["nodes"]["report"]["onError"] = {
            "retry": 0, "then": {"status": "paused", "reason": PAUSE_REASON}}
        registry["id"] = PAUSED_FLOW_ID
        task = registry["tasks"]["smoke-report"]
        task["tools"] = ["srv.tool"]
        task["outputContract"]["allowNoop"] = False
    flow_id = PAUSED_FLOW_ID if paused else FLOW_ID
    return {
        f"{flow_id}.flow.yaml": json_bytes(plan),
        f"{flow_id}.tasks.yaml": json_bytes(registry),
        PROMPT_FILE: b"TEST ONLY. Return a short smoke report for {{ user_input }}.\n",
    }


def build_bundle(*, corrupt_hash=False, paused=False):
    """Return deterministic uncompressed POSIX ustar bytes, regular files only."""
    files = fixture_files(paused=paused)
    flow_id = PAUSED_FLOW_ID if paused else FLOW_ID
    manifest = {
        "format": "trollm.flow-bundle.v1",
        "entryFlow": f"{flow_id}.flow.yaml",
        "taskRegistry": f"{flow_id}.tasks.yaml",
        "files": [{"path": path, "sha256": (
            "0" * 64 if corrupt_hash and path == PROMPT_FILE
            else hashlib.sha256(data).hexdigest())} for path, data in sorted(files.items())],
    }
    members = {"manifest.json": json_bytes(manifest), **files}
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for path, data in sorted(members.items()):
            info = tarfile.TarInfo(path)
            info.size = len(data)
            info.mode = 0o644
            # TarInfo defaults: uid/gid/mtime = 0, uname/gname = empty.
            archive.addfile(info, io.BytesIO(data))
    return output.getvalue()


def main():
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    valid = OUTPUT_DIR / f"{FLOW_ID}.tar"
    invalid = OUTPUT_DIR / f"{FLOW_ID}-bad-hash.tar"
    paused = OUTPUT_DIR / f"{PAUSED_FLOW_ID}.tar"
    valid.write_bytes(build_bundle())
    invalid.write_bytes(build_bundle(corrupt_hash=True))
    paused.write_bytes(build_bundle(paused=True))
    metadata = {
        "testOnly": True,
        "backend": "stub",
        "flowId": FLOW_ID,
        "flowSha256": hashlib.sha256(fixture_files()[FLOW_FILE]).hexdigest(),
        "userInput": USER_INPUT,
        "expectedPhase": "Succeeded",
        "expectedReport": EXPECTED_REPORT,
        "validBundle": valid.name,
        "badHashBundle": invalid.name,
        "expectedVerifiedFiles": len(fixture_files()),
        "pausedBundle": paused.name,
        "pausedFlowId": PAUSED_FLOW_ID,
        "pausedFlowSha256": hashlib.sha256(
            fixture_files(paused=True)[f"{PAUSED_FLOW_ID}.flow.yaml"]).hexdigest(),
        "expectedPauseReason": PAUSE_REASON,
        "expectedResumeEvent": "resumed",
        "expectedCancelPhase": "Failed",
        "expectedCancelReason": "cancelled",
        "expectedCancelOutcome": "cancelled",
    }
    (OUTPUT_DIR / "fixture.json").write_bytes(json_bytes(metadata))
    print(f"TEST ONLY: stub flow {FLOW_ID}; expected report: {EXPECTED_REPORT}")
    print(valid)
    print(invalid)
    print(paused)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
