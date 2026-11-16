import json
import unittest
from run_metrics import summarize


class Once:
    def __init__(self, records):
        self.lines = [json.dumps(record) + "\n" if not isinstance(record, str) else record for record in records]
        self.reads = 0

    def __iter__(self):
        self.reads += 1
        if self.reads != 1:
            raise AssertionError("journal reread")
        return iter(self.lines)


def context(epoch="private-epoch", attempt=1, call=None):
    value = {"epoch": epoch, "attempt": attempt}
    if call is not None:
        value["call"] = call
    return value


class RunMetricsTest(unittest.TestCase):
    def test_historical_public_phase_redactions_preserve_numeric_metrics(self):
        events = [
            {"type": "node_exited", "phase": "attempt", "attemptContext": context(),
             "measurement": {"activeNs": 123, "attemptInclusiveNs": 123}},
            {"type": "error", "phase": "tool_call", "callContext": context(call=1), "ms": 9},
        ]
        expected = summarize([Once(events)])
        events[0]["phase"] = "[redacted utf8Bytes=7 sha256=c7ce66d0fb14e3c2d4d920918cc6cc7e488668b08dddeb7b85e44de3b9f3eb01]"
        events[1]["phase"] = "[redacted utf8Bytes=9 sha256=99bd30a8e4c3e58fc1defdf5626b019fc311b42fae21005a5b4b551c2442ee54]"
        self.assertEqual(summarize([Once(events)]), expected)
        events[1]["phase"] = "[redacted utf8Bytes=8 sha256=99bd30a8e4c3e58fc1defdf5626b019fc311b42fae21005a5b4b551c2442ee54]"
        self.assertEqual(summarize([Once(events)])["counts"]["toolFailures"], 0)

    def test_parallel_preparation_is_not_added_to_foreground_time(self):
        cold = {"type": "model_call", "attemptContext": context(), "modelCallOrdinal": 1,
                "ms": 20, "diagnostics": {"preparationAsync": True,
                    "preparationWaitMs": 5, "weightVerifyMs": 50, "loadMs": 30}}
        warm = {"type": "model_call", "attemptContext": context(), "modelCallOrdinal": 2,
                "ms": 10, "diagnostics": {"generationMs": 8}}
        legacy = {"type": "model_call", "attemptContext": context(), "modelCallOrdinal": 3,
                  "ms": 7, "diagnostics": {"weightVerifyMs": 1, "loadMs": 2}}
        result = summarize([Once([cold, cold, warm, legacy])])
        self.assertEqual(result["counts"]["asyncPreparationModelCalls"], 1)
        self.assertEqual(result["counts"]["duplicateModelCalls"], 1)
        self.assertEqual(result["model"]["ms"]["sum"], 37)
        self.assertEqual(result["model"]["preparationWaitMs"]["sum"], 5)
        self.assertEqual(result["model"]["preparationWaitMs"]["unknown"], 2)
        self.assertEqual(result["model"]["weightVerifyMs"]["sum"], 51)
        self.assertEqual(result["model"]["loadMs"]["sum"], 32)

    def test_distinct_retries_identical_calls_epochs_failures_and_null_zero(self):
        main = Once([
            {"type": "model_call", "attemptContext": context(), "modelCallOrdinal": 1,
             "tokensIn": 10, "tokensOut": 0, "ms": 0, "requestPromptUtf8Bytes": 40,
             "diagnostics": {"prefillMs": 0, "generationMs": None}},
            {"type": "tool_call_result", "attemptContext": context(),
             "callContext": context(call=1), "ms": 2},
            {"type": "tool_call_result", "attemptContext": context(),
             "callContext": context(call=2), "ms": 3},
            {"type": "node_exited", "phase": "attempt", "attemptContext": context(),
             "measurement": {"attemptInclusiveNs": 12, "userHoldNs": 0}},
            {"type": "model_call", "attemptContext": context(attempt=2), "modelCallOrdinal": 1,
             "failed": True, "tokensIn": None, "tokensOut": None, "ms": 5,
             "diagnostics": {"prefillMs": 3}},
            {"type": "error", "phase": "tool_call", "callContext": context(attempt=2, call=1), "ms": 9},
            {"type": "tool_call_result", "callContext": context(epoch="new-private-epoch", call=1), "ms": 1},
        ])
        proxy = Once([
            {"type": "response_size", "seq": 1, "runId": "private-run", "stage": "raw_page",
             "call_context": context(call=1), "compact_json_utf8_bytes": 20, "timings": {"elapsedMs": 1}},
            {"type": "response_size", "seq": 2, "runId": "private-run", "stage": "gateway_result",
             "call_context": context(call=1), "compact_json_utf8_bytes": 8,
             "timings": {"elapsedMs": 2, "transportMs": 0}},
            {"type": "response_size", "seq": 3, "runId": "private-run", "stage": "failed_result",
             "call_context": context(attempt=2, call=1), "compact_json_utf8_bytes": None,
             "timings": {"elapsedMs": 9, "transportMs": 8}},
        ])
        result = summarize([main, proxy])
        self.assertEqual((main.reads, proxy.reads), (1, 1))
        self.assertEqual(result["counts"]["toolCalls"], 4)
        self.assertEqual(result["counts"]["toolFailures"], 1)
        self.assertEqual(result["counts"]["modelFailures"], 1)
        self.assertEqual(result["counts"]["incompleteAttempts"], 1)
        self.assertEqual(result["counts"]["matchedMainProxyCalls"], 2)
        self.assertEqual(result["model"]["tokensIn"]["unknown"], 1)
        self.assertEqual(result["model"]["tokensOut"]["zero"], 1)
        self.assertEqual(result["gateway"]["elapsedMs"]["sum"], 11)  # excludes intermediate page
        self.assertEqual(result["gateway"]["transportMs"]["zero"], 1)
        self.assertEqual(result["proxySize"]["failed_result"]["unknown"], 1)
        rendered = json.dumps(result)
        for secret in ("private-epoch", "new-private-epoch", "private-run"):
            self.assertNotIn(secret, rendered)

    def test_duplicate_unknown_malformed_and_private_content(self):
        event = {"type": "model_call", "attemptContext": context(), "modelCallOrdinal": 1,
                 "ms": 1, "tokensIn": 7, "prompt": "TOP-SECRET", "server": "https://private"}
        attempt = {"type": "node_exited", "phase": "attempt", "attemptContext": context(),
                   "measurement": {"activeNs": 0, "userHoldNs": None}}
        proxy = {"type": "response_size", "seq": 1, "runId": "private", "stage": "gateway_result",
                 "compact_json_utf8_bytes": 0, "call_context": context(call=1)}
        result = summarize([Once([event, event, attempt, attempt, proxy, proxy,
                                 {"type": "model_call", "ms": -1, "tokensIn": True},
                                 {"type": "error", "phase": "tool_call"}, "bad\n", []])])
        counts = result["counts"]
        self.assertEqual(counts["duplicateModelCalls"], 1)
        self.assertEqual(counts["duplicateAttemptMeasurements"], 1)
        self.assertEqual(counts["duplicateProxyObservations"], 1)
        self.assertEqual(counts["unknownModelIdentity"], 1)
        self.assertEqual(counts["unknownToolIdentity"], 1)
        self.assertEqual(counts["malformedRecords"], 2)
        self.assertEqual(result["model"]["ms"]["unknown"], 1)
        self.assertEqual(result["model"]["tokensIn"]["known"], 1)  # bool isn't numeric evidence
        self.assertEqual(result["attempt"]["activeNs"]["zero"], 1)
        self.assertEqual(result["attempt"]["userHoldNs"]["unknown"], 1)
        self.assertNotIn("TOP-SECRET", json.dumps(result))
        self.assertNotIn("https://private", json.dumps(result))

    def test_terminal_measurement_is_explicitly_partial(self):
        result = summarize([Once([{
            "type":"run_ended", "attemptContext":context(),
            "measurement":{"attemptObservedNs":20,"attemptInclusiveNs":None,
                           "activeNs":None,"boundarySnapshotIncluded":False},
        }])])
        self.assertEqual(result["counts"]["attemptMeasurements"],1)
        self.assertEqual(result["counts"]["partialAttemptMeasurements"],1)
        self.assertEqual(result["counts"]["incompleteAttempts"],0)
        self.assertEqual(result["attempt"]["attemptObservedNs"]["sum"],20)
        self.assertEqual(result["attempt"]["attemptInclusiveNs"]["unknown"],1)

    def test_interrupted_mutation_keeps_intent_unresolved(self):
        result = summarize([Once([
            {"type":"tool_call_intent", "attemptContext":context(), "callContext":context(call=1)},
            {"type":"response_size", "stage":"raw_payload", "call_context":context(call=1), "compact_json_utf8_bytes":0},
        ])])
        self.assertEqual(result["counts"]["incompleteAttempts"], 1)
        self.assertEqual(result["counts"]["toolIntents"], 1)
        self.assertEqual(result["counts"]["incompleteToolCalls"], 1)
        self.assertEqual(result["counts"]["toolCalls"], 0)
        self.assertEqual(result["counts"]["matchedMainProxyCalls"], 1)

    def test_large_journal_and_observer_count_do_not_multiply_reads(self):
        stream = Once([{"type": "node_exited", "phase": "attempt", "attemptContext": context(attempt=i),
                        "measurement": {"attemptInclusiveNs": i}}
                       for i in range(1, 10001)])
        result = summarize([stream])
        self.assertEqual(stream.reads, 1)
        self.assertEqual(result["counts"]["attemptMeasurements"], 10000)
        self.assertEqual(result["attempt"]["attemptInclusiveNs"]["sum"], 50005000)


if __name__ == "__main__":
    unittest.main()
