"""Read-only exact-response check for an explicitly selected observation run.

The operator must identify the cache generation belonging to the selected run.
Raw evidence stays in memory; reports contain only status and the checked count.
"""

import argparse
import json
import os
from pathlib import Path
import re
import sys
import uuid

from make_native_probe_bundle import OBSERVED_FLOW_ID
from smoke import command, SmokeError

MAX_BYTES = 64 * 1024
EMPTY_ARGS_HASH = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
CACHE_PATTERN = re.compile(
    rf"trollm/cache/mcp/gateway-[0-9]+-[0-9]+-[0-9]+/e2llm/list_tabs/{EMPTY_ARGS_HASH}\.jsonl"
)


class ProbeCheckError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeCheckError(message)


def valid_run_id(run_id):
    try:
        return str(uuid.UUID(run_id)) == run_id
    except (ValueError, TypeError, AttributeError):
        return False


def verify_probe(run_id, snapshot, events, cache_jsonl):
    """Verify decoded run evidence and one raw whole-payload cache JSONL value."""
    require(valid_run_id(run_id), "invalid run identity")
    require(isinstance(snapshot, dict) and snapshot.get("runId") == run_id,
            "snapshot run identity mismatch")
    require(snapshot.get("status") == "succeeded", "run is not succeeded")
    stack = snapshot.get("stack")
    require(isinstance(stack, list) and len(stack) == 1 and isinstance(stack[0], dict),
            "invalid root frame")
    frame = stack[0]
    require(isinstance(frame.get("plan"), str) and re.fullmatch(
        re.escape(OBSERVED_FLOW_ID) + r"@sha256:[0-9a-f]{64}", frame["plan"]),
        "unexpected observation flow")
    require(isinstance(events, list) and bool(events)
            and all(isinstance(event, dict) and event.get("runId") == run_id for event in events),
            "event run identity mismatch")
    sequences = [event.get("seq") for event in events]
    require(all(type(seq) is int and seq > 0 for seq in sequences)
            and sequences == list(range(1, len(events) + 1)), "invalid event sequence")
    require(snapshot.get("eventWatermark") == sequences[-1], "snapshot event boundary mismatch")
    require(not any(event.get("type") == "error" for event in events), "run contains an error")
    starts = [event for event in events if event.get("type") == "run_started"]
    require(len(starts) == 1 and starts[0] is events[0]
            and starts[0].get("plan") == OBSERVED_FLOW_ID
            and starts[0].get("sha256") == frame["plan"].split("@sha256:")[1],
            "event flow identity mismatch")
    endings = [event for event in events if event.get("type") == "run_ended"]
    require(len(endings) == 1 and endings[0] is events[-1]
            and endings[0].get("outcome") == "succeeded", "missing terminal success")
    models = [event for event in events if event.get("type") == "model_call"]
    calls = [event for event in events if event.get("type") == "tool_call_result"]
    require(len(models) == 2 and all(event.get("slot") == "coder" and event.get("node") == "probe"
                                   for event in models), "unexpected model generations")
    require(len(calls) == 1 and calls[0].get("server") == "e2llm"
            and calls[0].get("tool") == "list_tabs" and calls[0].get("node") == "probe"
            and calls[0].get("provenance") == "Live",
            "missing expected live tool call")
    require(models[0]["seq"] < calls[0]["seq"] < models[1]["seq"], "unexpected call ordering")
    try:
        lines = cache_jsonl.splitlines()
        require(len(lines) == 1, "invalid cached payload")
        payload = json.loads(lines[0])
    except (ValueError, TypeError, AttributeError):
        raise ProbeCheckError("invalid cached payload") from None
    require(isinstance(payload, dict) and isinstance(payload.get("tabs"), list),
            "invalid cached tabs array")
    outputs = frame.get("nodeOutputs")
    require(isinstance(outputs, dict) and isinstance(outputs.get("probe"), dict),
            "missing probe output")
    require(outputs["probe"].get("report") == str(len(payload["tabs"])), "exact response mismatch")
    return {"status": "passed", "tabCount": len(payload["tabs"])}


def read_private(adb, serial, socket, path):
    text = command([adb, "-s", serial, "exec-out", "run-as", "com.trollm.app",
                    "head", "-c", str(MAX_BYTES + 1), "files/" + path],
                   env={**os.environ, "ADB_SERVER_SOCKET": socket})
    require(len(text.encode("utf-8")) <= MAX_BYTES, "evidence exceeds size limit")
    return text


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--cache-entry", required=True,
                        help="Explicit app-private gateway JSONL path observed for this run")
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args(argv)
    try:
        require(bool(re.fullmatch(r"[A-Za-z0-9_.:\[\]-]+", args.serial)), "invalid device selection")
        require(args.socket.startswith(("tcp:", "localfilesystem:", "localabstract:"))
                and not any(char.isspace() for char in args.socket), "invalid ADB socket")
        require(valid_run_id(args.run_id), "invalid run identity")
        require(bool(CACHE_PATTERN.fullmatch(args.cache_entry)), "invalid cache selection")
        prefix = f"trollm/runs/{args.run_id}"
        snapshot_text = read_private(args.adb, args.serial, args.socket, prefix + "/state.snapshot.json")
        events_text = read_private(args.adb, args.serial, args.socket, prefix + "/events.jsonl")
        cache_text = read_private(args.adb, args.serial, args.socket, args.cache_entry)
        snapshot = json.loads(snapshot_text)
        events = [json.loads(line) for line in events_text.splitlines()]
        result = verify_probe(args.run_id, snapshot, events, cache_text)
    except ProbeCheckError as error:
        result = {"status": "failed", "error": str(error)}
    except (SmokeError, ValueError, TypeError, KeyError, OSError, UnicodeError):
        result = {"status": "failed", "error": "unable to verify bounded probe evidence"}
    if args.report is not None:
        try:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(result, sort_keys=True) + "\n")
        except OSError:
            result = {"status": "failed", "error": "unable to write sanitized report"}
    print(json.dumps(result, sort_keys=True))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
