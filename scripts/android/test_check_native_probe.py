"""Synthetic evidence only: no device, browser payload or network access."""

import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import check_native_probe as check

RUN = "12345678-1234-4234-8234-123456789abc"
CACHE = f"trollm/cache/mcp/gateway-123-456-0/e2llm/list_tabs/{check.EMPTY_ARGS_HASH}.jsonl"


def evidence(count=3):
    snapshot = {"runId": RUN, "status": "succeeded", "eventWatermark": 5, "stack": [{
        "plan": f"{check.OBSERVED_FLOW_ID}@sha256:" + "a" * 64,
        "nodeOutputs": {"probe": {"report": str(count)}},
    }]}
    events = [
        {"type": "run_started", "plan": check.OBSERVED_FLOW_ID, "sha256": "a" * 64},
        {"type": "model_call", "slot": "coder", "node": "probe"},
        {"type": "tool_call_result", "server": "e2llm", "tool": "list_tabs", "node": "probe", "provenance": "Live"},
        {"type": "model_call", "slot": "coder", "node": "probe"},
        {"type": "run_ended", "outcome": "succeeded"},
    ]
    for seq, event in enumerate(events, 1):
        event.update(runId=RUN, seq=seq)
    payload = json.dumps({"tabs": [{"title": "WITHHELD_TEST_DATA", "url": "https://example.test/private"}] * count}) + "\n"
    return snapshot, events, payload


class NativeProbeCheck(unittest.TestCase):
    def test_zero_and_three_tabs_pass_exactly(self):
        for count in (0, 3):
            self.assertEqual(check.verify_probe(RUN, *evidence(count)),
                             {"status": "passed", "tabCount": count})

    def test_succeeded_run_with_wrong_or_noncanonical_report_fails(self):
        for report in ("2", "03", "3.", " 3", 3):
            snapshot, events, payload = evidence()
            snapshot["stack"][0]["nodeOutputs"]["probe"]["report"] = report
            with self.assertRaisesRegex(check.ProbeCheckError, "exact response mismatch"):
                check.verify_probe(RUN, snapshot, events, payload)

    def test_missing_wrong_or_nonlive_call_and_generation_order_fail(self):
        for field, value in (("provenance", "Cache"), ("tool", "other"), ("server", "other"),
                             ("node", "other"), ("type", "unrelated")):
            snapshot, events, payload = evidence()
            events[2][field] = value
            with self.assertRaises(check.ProbeCheckError):
                check.verify_probe(RUN, snapshot, events, payload)
        snapshot, events, payload = evidence()
        events[1], events[2] = events[2], events[1]
        for seq, event in enumerate(events, 1):
            event["seq"] = seq
        with self.assertRaisesRegex(check.ProbeCheckError, "ordering"):
            check.verify_probe(RUN, snapshot, events, payload)

    def test_wrong_run_nonterminal_error_flow_and_snapshot_boundary_fail(self):
        snapshot, events, payload = evidence()
        cases = []
        for field, value in (("runId", "other"), ("status", "running"), ("eventWatermark", 4)):
            changed = copy.deepcopy(snapshot)
            changed[field] = value
            cases.append((changed, events, payload))
        changed = copy.deepcopy(snapshot)
        changed["stack"][0]["plan"] = "other@sha256:" + "a" * 64
        cases.append((changed, events, payload))
        for field, value in (("runId", "other"), ("type", "error"), ("outcome", "failed")):
            changed = copy.deepcopy(events)
            changed[-1][field] = value
            cases.append((snapshot, changed, payload))
        for values in cases:
            with self.assertRaises(check.ProbeCheckError):
                check.verify_probe(RUN, *values)

    def test_invalid_payload_shapes_are_not_zero_tabs(self):
        snapshot, events, _ = evidence(0)
        for payload in ("", "bad JSON", "[]\n", "{}\n", '{"tabs":null}\n',
                        '{"tabs":{}}\n', '{"tabs":[]}\n{}\n'):
            with self.assertRaises(check.ProbeCheckError):
                check.verify_probe(RUN, snapshot, events, payload)

    def test_cli_reads_only_three_bounded_paths_and_outputs_no_browser_content(self):
        snapshot, events, payload = evidence()
        reads = [json.dumps(snapshot), "\n".join(map(json.dumps, events)) + "\n", payload]
        with patch.object(check, "command", side_effect=reads) as command, contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(check.main(self.args()), 0)
        self.assertEqual(json.loads(out.getvalue()), {"status": "passed", "tabCount": 3})
        self.assertEqual(command.call_count, 3)
        self.assertEqual([call.args[0][-1] for call in command.call_args_list],
                         [f"files/trollm/runs/{RUN}/state.snapshot.json",
                          f"files/trollm/runs/{RUN}/events.jsonl", "files/" + CACHE])
        for call in command.call_args_list:
            self.assertEqual(call.args[0][3:9], ["exec-out", "run-as", "com.trollm.app", "head", "-c", "65537"])
            self.assertEqual(call.kwargs["env"]["ADB_SERVER_SOCKET"], "tcp:localhost:5037")

    def args(self):
        return ["--serial", "emulator-5554", "--socket", "tcp:localhost:5037",
                "--run-id", RUN, "--cache-entry", CACHE]

    def test_cli_rejects_bad_selectors_before_adb(self):
        for option, value in (("--run-id", "../bad"), ("--cache-entry", CACHE.replace("gateway-123-456-0", "..")),
                              ("--cache-entry", CACHE.replace(check.EMPTY_ARGS_HASH, "a" * 64)),
                              ("--cache-entry", "/" + CACHE), ("--serial", "bad;command")):
            args = self.args()
            args[args.index(option) + 1] = value
            with patch.object(check, "command") as command, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(check.main(args), 1)
                command.assert_not_called()

    def test_cli_failures_and_report_are_sanitized(self):
        with tempfile.TemporaryDirectory() as folder:
            report = Path(folder) / "result.json"
            with patch.object(check, "command", side_effect=check.SmokeError("WITHHELD_TEST_DATA")), contextlib.redirect_stdout(io.StringIO()) as out:
                self.assertEqual(check.main(self.args() + ["--report", str(report)]), 1)
            self.assertNotIn("WITHHELD_TEST_DATA", out.getvalue() + report.read_text())
            self.assertEqual(json.loads(report.read_text())["error"], "unable to verify bounded probe evidence")
        with patch.object(check, "command", return_value="x" * (check.MAX_BYTES + 1)), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(check.main(self.args()), 1)
        self.assertEqual(json.loads(out.getvalue())["error"], "evidence exceeds size limit")


if __name__ == "__main__":
    unittest.main()
