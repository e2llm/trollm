#!/usr/bin/env python3
"""Offline numeric run observations. Read each supplied JSONL journal once.

Inputs stay private. Output contains fixed aggregate names and numbers only;
no run/call identities, prompt/tool content, hashes, paths or endpoints. Phase
and size boundaries overlap: this tool never adds different boundaries together.
This is an observer, not a recovery validator or execution authority.

directToolSteps is present only when direct_tool_step events occur. It counts
charged direct attempts, including failures before tool dispatch, independently
of modelCalls and toolCalls. Optional duplicate/unknown-identity counts accompany
that field; historical summaries without direct events retain their exact shape.
"""
import argparse
import hashlib
import json
import sys

ATTEMPT_FIELDS = (
    "attemptObservedNs", "attemptInclusiveNs", "userHoldNs", "retryWaitNs", "activeObservedNs", "activeNs",
    "eventRecords", "eventRecordBytes", "eventSerializeNs", "eventWriteFlushNs",
    "eventObserverNs", "snapshotAttempts", "snapshotWrites", "snapshotBytes",
    "snapshotSerializeNs", "snapshotWriteRenameNs",
)
MODEL_FIELDS = ("ms", "tokensIn", "tokensOut", "requestPromptUtf8Bytes", "promptAssemblyNs", "promptRenderNs")
MODEL_PHASES = ("totalMs", "weightVerifyMs", "loadMs", "preparationWaitMs", "prepareMs", "prefillMs", "generationMs")
GATEWAY_PHASES = ("elapsedMs", "transportMs", "cacheReadMs", "resultAdmissionMs", "resultFinishMs")
SIZE_STAGES = (
    "raw_payload", "raw_page", "raw_error_payload", "complete_collection",
    "selected_page", "gateway_result", "failed_result",
)
PUBLIC_PHASE_MARKERS = {
    f"[redacted utf8Bytes={len(value)} sha256={hashlib.sha256(value.encode()).hexdigest()}]": value
    for value in ("attempt", "tool_call")
}


def phase(event):
    """Recognize only public runtime phase constants in historical redactions.

    This does not recover page content, model prose or arbitrary private values.
    The archived bytes and all measured numbers remain unchanged.
    """
    value = event.get("phase")
    return PUBLIC_PHASE_MARKERS.get(value, value) if isinstance(value, str) else value


def integer(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


class Stats:
    def __init__(self):
        self.known = self.unknown = self.zero = self.total = 0
        self.minimum = self.maximum = None

    def add(self, value):
        if not integer(value):
            self.unknown += 1
            return
        self.known += 1
        self.zero += value == 0
        self.total += value
        self.minimum = value if self.minimum is None else min(self.minimum, value)
        self.maximum = value if self.maximum is None else max(self.maximum, value)

    def value(self):
        return {"known": self.known, "unknown": self.unknown, "zero": self.zero,
                "sum": self.total, "min": self.minimum, "max": self.maximum}


def context_key(context, call=False):
    if not isinstance(context, dict):
        return None
    epoch, attempt = context.get("epoch"), context.get("attempt")
    if not isinstance(epoch, str) or not epoch or not integer(attempt) or attempt == 0:
        return None
    if call:
        ordinal = context.get("call")
        return (epoch, attempt, ordinal) if integer(ordinal) and ordinal > 0 else None
    return epoch, attempt


def summarize(streams):
    """Consume each iterable once, keeping identifiers only for private deduplication."""
    counts = {name: 0 for name in (
        "records", "malformedRecords", "attempts", "attemptMeasurements", "incompleteAttempts",
        "duplicateAttemptMeasurements", "partialAttemptMeasurements", "modelCalls", "modelFailures", "duplicateModelCalls",
        "asyncPreparationModelCalls",
        "toolCalls", "toolFailures", "toolIntents", "incompleteToolCalls", "duplicateToolCalls", "proxyObservations",
        "unknownAttemptIdentity", "unknownModelIdentity", "unknownToolIdentity",
        "unknownProxyIdentity", "unknownToolIntentIdentity", "matchedMainProxyCalls", "mainCallsWithoutProxy", "proxyCallsWithoutMain",
        "duplicateProxyObservations", "unknownSizeStages",
    )}
    attempts, measured, models, tools, proxy_calls, proxy_records = set(), set(), set(), set(), set(), set()
    intents = set()
    direct_steps = set()
    attempt_stats = {key: Stats() for key in ATTEMPT_FIELDS}
    model_stats = {key: Stats() for key in MODEL_FIELDS + MODEL_PHASES}
    tool_stats = {"ms": Stats(), "gatewayResultCompactJsonUtf8Bytes": Stats(), "toolResultContextCandidateUtf8Bytes": Stats()}
    size_stats = {stage: Stats() for stage in SIZE_STAGES}
    gateway_stats = {key: Stats() for key in GATEWAY_PHASES}
    for stream in streams:
        for line in stream:
            if not line.strip():
                continue
            try:
                event = json.loads(line)
            except (ValueError, UnicodeError):
                counts["malformedRecords"] += 1
                continue
            if not isinstance(event, dict):
                counts["malformedRecords"] += 1
                continue
            counts["records"] += 1
            ty = event.get("type")
            attempt = context_key(event.get("attemptContext"))
            if attempt:
                attempts.add(attempt)
            if (ty == "node_exited" and phase(event) == "attempt") or (
                    ty in ("run_ended", "paused") and isinstance(event.get("measurement"), dict)):
                if attempt is None:
                    counts["unknownAttemptIdentity"] += 1
                elif attempt in measured:
                    counts["duplicateAttemptMeasurements"] += 1
                    continue
                else:
                    measured.add(attempt)
                counts["attemptMeasurements"] += 1
                measurement = event.get("measurement")
                measurement = measurement if isinstance(measurement, dict) else {}
                counts["partialAttemptMeasurements"] += measurement.get("boundarySnapshotIncluded") is False
                for key, stats in attempt_stats.items():
                    stats.add(measurement.get(key))
            elif ty == "direct_tool_step":
                counts.setdefault("directToolSteps", 0)
                counts.setdefault("duplicateDirectToolSteps", 0)
                counts.setdefault("unknownDirectIdentity", 0)
                if attempt is None:
                    counts["unknownDirectIdentity"] += 1
                elif attempt in direct_steps:
                    counts["duplicateDirectToolSteps"] += 1
                    continue
                else:
                    direct_steps.add(attempt)
                counts["directToolSteps"] += 1
            elif ty == "model_call":
                ordinal = event.get("modelCallOrdinal")
                model = (*attempt, ordinal) if attempt and integer(ordinal) and ordinal > 0 else None
                if model is None:
                    counts["unknownModelIdentity"] += 1
                elif model in models:
                    counts["duplicateModelCalls"] += 1
                    continue
                else:
                    models.add(model)
                counts["modelCalls"] += 1
                counts["modelFailures"] += event.get("failed") is True or event.get("finishReason") in ("timeout", "length", "grammar-reject")
                diagnostics = event.get("diagnostics")
                diagnostics = diagnostics if isinstance(diagnostics, dict) else {}
                # Preparation may run alongside another model/tool call. Keep
                # phase totals separate; only the foreground wait is inside ms.
                counts["asyncPreparationModelCalls"] += diagnostics.get("preparationAsync") is True
                for key in MODEL_FIELDS:
                    model_stats[key].add(event.get(key))
                for key in MODEL_PHASES:
                    model_stats[key].add(diagnostics.get(key))
            elif ty == "tool_call_intent":
                call = context_key(event.get("callContext"), call=True)
                if call is None:
                    counts["unknownToolIntentIdentity"] += 1
                else:
                    intents.add(call)
            elif ty == "tool_call_result" or (ty == "error" and phase(event) == "tool_call"):
                call = context_key(event.get("callContext"), call=True)
                if call is None:
                    counts["unknownToolIdentity"] += 1
                elif call in tools:
                    counts["duplicateToolCalls"] += 1
                    continue
                else:
                    tools.add(call)
                counts["toolCalls"] += 1
                counts["toolFailures"] += ty == "error"
                for key, stats in tool_stats.items():
                    stats.add(event.get(key))
            elif ty == "response_size":
                call = context_key(event.get("call_context"), call=True)
                if call is None:
                    counts["unknownProxyIdentity"] += 1
                else:
                    proxy_calls.add(call)
                # A journal-local record key handles repeated input files without
                # conflating multiple pages/stages belonging to the same call.
                seq, run = event.get("seq"), event.get("runId")
                record = (run, seq) if isinstance(run, str) and integer(seq) else None
                if record and record in proxy_records:
                    counts["duplicateProxyObservations"] += 1
                    continue
                if record:
                    proxy_records.add(record)
                counts["proxyObservations"] += 1
                stage = event.get("stage")
                if isinstance(stage, str) and stage in size_stats:
                    size_stats[stage].add(event.get("compact_json_utf8_bytes"))
                else:
                    counts["unknownSizeStages"] += 1
                # Timings are cumulative within one call: retain only the final
                # gateway_result/failed_result boundary, never sum intermediate
                # page envelopes as independent gateway durations.
                if stage in ("gateway_result", "failed_result"):
                    timing = event.get("timings")
                    timing = timing if isinstance(timing, dict) else {}
                    for key, stats in gateway_stats.items():
                        stats.add(timing.get(key))
    counts["attempts"] = len(attempts)
    counts["incompleteAttempts"] = len(attempts - measured)
    main_calls = tools | intents
    counts["toolIntents"] = len(intents)
    counts["incompleteToolCalls"] = len(intents - tools)
    counts["matchedMainProxyCalls"] = len(main_calls & proxy_calls)
    counts["mainCallsWithoutProxy"] = len(main_calls - proxy_calls)
    counts["proxyCallsWithoutMain"] = len(proxy_calls - main_calls)
    return {"schemaVersion": 1, "counts": counts,
            "attempt": {key: stats.value() for key, stats in attempt_stats.items()},
            "model": {key: stats.value() for key, stats in model_stats.items()},
            "tool": {key: stats.value() for key, stats in tool_stats.items()},
            "proxySize": {key: stats.value() for key, stats in size_stats.items()},
            "gateway": {key: stats.value() for key, stats in gateway_stats.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("journals", nargs="+", help="private main/proxy JSONL files to read once")
    args = parser.parse_args()
    # ExitStack opens each explicit input once; no polling or automatic upload.
    from contextlib import ExitStack
    try:
        with ExitStack() as stack:
            streams = [stack.enter_context(open(path, encoding="utf-8")) for path in args.journals]
            result = summarize(streams)
    except (OSError, UnicodeError):
        print("could not read a supplied journal", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
