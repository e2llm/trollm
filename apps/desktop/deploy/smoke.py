#!/usr/bin/env python3
"""Check a running desktop instance with disposable data."""

import argparse
from collections import Counter
from contextlib import suppress
import hashlib
import http.client
import io
import json
import math
import os
import re
import sys
import tarfile
import time
from urllib.parse import quote, urlsplit
import uuid


MAX_RESPONSE_BYTES = 4 * 1024 * 1024
HTTP_TIMEOUT = 10
DIRECT_ID = "desktop-http-smoke"
SUMMARY_INPUT = (
    "The community garden opens on Saturday. Volunteers will water the new "
    "seedlings and label each bed before the visitors arrive."
)


class SmokeError(Exception):
    """An acceptance check failed."""


class TransportError(SmokeError):
    """The service could not be reached."""


def require(condition, explanation):
    if not condition:
        raise SmokeError(explanation)


class DesktopClient:
    def __init__(self, url):
        require(not re.search(r"[\s\\]", url), "Use the configured HTTP(S) origin as --url.")
        try:
            origin = urlsplit(url)
            port = origin.port
            host = origin.hostname
        except ValueError:
            raise SmokeError("Use the configured HTTP(S) origin as --url.") from None
        require(
            origin.scheme in ("http", "https") and host and origin.username is None
            and origin.password is None and origin.path in ("", "/")
            and not origin.query and not origin.fragment,
            "Use the configured HTTP(S) origin as --url.",
        )
        self.origin = f"{origin.scheme}://{origin.netloc}"
        self.authority = origin.netloc
        self.host = host
        self.port = port
        self.connection = (
            http.client.HTTPSConnection if origin.scheme == "https" else http.client.HTTPConnection
        )
        self.token = ""

    def read(
        self, path, *, method="GET", body=None, content_type=None, status=200,
        origin=None, authority=None, authorized=True, timeout=HTTP_TIMEOUT,
    ):
        headers = {"Host": self.authority if authority is None else authority}
        if origin != "":
            headers["Origin"] = self.origin if origin is None else origin
        if authorized and self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if content_type:
            headers["Content-Type"] = content_type

        # http.client connects directly and does not follow redirects.
        connection = self.connection(self.host, self.port, timeout=timeout)
        try:
            connection.request(method, path, body=body, headers=headers)
            with connection.getresponse() as response:
                require(response.status == status, f"{method} {path} returned HTTP {response.status}; expected {status}.")
                data = response.read(MAX_RESPONSE_BYTES + 1)
                require(len(data) <= MAX_RESPONSE_BYTES, "A response exceeded the smoke probe's size limit.")
                return data, response.headers
        except (OSError, http.client.HTTPException):
            raise TransportError(f"Could not read the response to {method} {path}.") from None
        finally:
            connection.close()

    def json(self, path, *, fields=None, **options):
        if fields is not None:
            options.update(body=json.dumps(fields).encode(), content_type="application/json")
        data, headers = self.read(path, **options)
        require(headers.get_content_type() == "application/json", "Expected a JSON service response.")
        if path.startswith("/api/"):
            require(headers.get("Cache-Control") == "no-store", "An API response did not prevent caching.")
        try:
            document = json.loads(data)
        except (UnicodeError, ValueError):
            raise SmokeError("Received malformed JSON from the desktop service.") from None
        require(isinstance(document, dict), "Expected an object from the desktop service.")
        return document


def error_code(document, expected):
    require(document.get("error", {}).get("code") == expected, "The API returned an unexpected error category.")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def calculator_bundle():
    entry = f"{DIRECT_ID}.flow.yaml"
    registry = f"{DIRECT_ID}.tasks.yaml"
    setup = json.dumps({
        "format": "trollm.flow-setup.v1",
        "title": "Check calculator result",
        "models": {},
        "connections": {},
    }).encode()
    plan = f"""\
schemaVersion: ledger.execution-plan.v1
id: {DIRECT_ID}
version: 1
setup: {{path: setup.json, sha256: "{digest(setup)}"}}
limits: {{maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 30, artifactMaxMb: 1}}
onLimit: abort
models: {{}}
mcp:
  policies:
    local.calculate: {{class: read, resultArtifact: calculation}}
    local.assert_equal: {{class: read, resultArtifact: proof}}
entryStep: calculate
nodes:
  calculate: {{kind: task, taskRef: add, onVerified: verify}}
  verify: {{kind: task, taskRef: check-result, onVerified: done}}
  done: {{kind: end, outcome: succeeded}}
""".encode()
    tasks = b"""\
schemaVersion: ledger.task-registry.v1
tasks:
  add:
    execution: tool
    completion: tool-result
    tools: [local.calculate]
    toolArguments:
      local.calculate:
        modelFields: []
        fixed: {operation: add, left: '39.99', right: '40.10'}
    outputContract:
      outputs: {calculation: {fromArtifact: calculation, format: json, type: object}}
      requireArtifactComplete: [calculation]
  check-result:
    execution: tool
    completion: tool-result
    tools: [local.assert_equal]
    toolArguments:
      local.assert_equal:
        modelFields: []
        fromVars: {left: calculate.outputs.calculation.result}
        fixed: {right: '80.09'}
    outputContract:
      outputs: {proof: {fromArtifact: proof, format: json, type: object}}
      requireArtifactComplete: [proof]
"""
    members = {entry: plan, registry: tasks, "setup.json": setup}
    manifest = json.dumps({
        "format": "trollm.flow-bundle.v1",
        "entryFlow": entry,
        "taskRegistry": registry,
        "files": [{"path": path, "sha256": digest(data)} for path, data in members.items()],
    }).encode()
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for path, data in {"manifest.json": manifest, **members}.items():
            info = tarfile.TarInfo(path)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return archive.getvalue(), digest(plan)


def check_health_and_ui(client, backend):
    deadline = time.monotonic() + 20
    while True:
        try:
            health = client.json("/healthz", timeout=min(HTTP_TIMEOUT, max(0.1, deadline - time.monotonic())))
            break
        except TransportError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.25)
    require(health.get("status") == "ok" and health.get("backend") == backend, "Unexpected desktop health or backend.")
    html, headers = client.read("/")
    require(headers.get_content_type() == "text/html" and b'id="root"' in html, "The desktop HTML shell was not served.")
    asset = re.search(rb'<script\b[^>]*\bsrc="(/assets/[A-Za-z0-9_-]+\.js)"', html)
    require(asset, "The desktop HTML did not reference a built JavaScript asset.")
    script, headers = client.read(asset[1].decode("ascii"))
    require("javascript" in headers.get_content_type() and script, "The built desktop JavaScript was not served.")
    stylesheet = re.search(rb'<link\b[^>]*\bhref="(/assets/[A-Za-z0-9_-]+\.css)"', html)
    require(stylesheet, "The desktop HTML did not reference a built stylesheet.")
    css, headers = client.read(stylesheet[1].decode("ascii"))
    require(headers.get_content_type() == "text/css" and css, "The built desktop stylesheet was not served.")


def check_public_guards(client, backend):
    error_code(client.json("/api/flows", status=401), "SESSION_REQUIRED")
    error_code(client.json("/api/runs", status=401), "SESSION_REQUIRED")
    error_code(client.json("/api/bootstrap", authority="outside.example.invalid", origin="", status=403), "ORIGIN_DENIED")
    error_code(client.json("/api/bootstrap", origin="https://outside.example.invalid", status=403), "ORIGIN_DENIED")

    bootstrap = client.json("/api/bootstrap")
    require(bootstrap.get("backend") == backend and isinstance(bootstrap.get("needsSetup"), bool)
            and isinstance(bootstrap.get("version"), str) and bootstrap["version"],
            "The bootstrap response did not describe a usable desktop host.")


def unlock(client, passphrase):
    session = client.json("/api/session", method="POST", fields={"passphrase": passphrase})
    token = session.get("token")
    require(isinstance(token, str) and re.fullmatch(r"[A-Za-z0-9_-]{43}", token),
            "Vault unlock did not return a session token.")
    client.token = token


def start(client, flow_sha, input_text, *, check_repeat=False):
    payload = {"requestId": str(uuid.uuid4()), "flowSha256": flow_sha, "input": input_text}
    response = client.json("/api/runs", method="POST", fields=payload)
    run_id = response.get("id")
    require(isinstance(run_id, str) and re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id),
            "An explicit Start did not return a run ID.")
    if check_repeat:
        duplicate = client.json("/api/runs", method="POST", fields=payload)
        require(duplicate.get("id") == run_id, "Repeating the same Start request changed its run ID.")
    return run_id


def wait_for_run(client, run_id, flow_sha, timeout):
    deadline = time.monotonic() + timeout
    after = 0
    counts = Counter()
    while time.monotonic() < deadline:
        page = client.json(
            f"/api/runs/{run_id}?after={after}",
            timeout=min(HTTP_TIMEOUT, max(0.1, deadline - time.monotonic())),
        )
        run = page["run"]
        events = page["events"]
        next_seq = page["nextSeq"]
        require(run.get("id") == run_id and run.get("flowSha256") == flow_sha,
                "The run observation did not match the requested run and flow.")
        require(type(next_seq) is int and next_seq >= after and isinstance(page["hasMore"], bool)
                and len(events) <= 129, "The bounded activity page was invalid.")
        event_seq = [event["seq"] for event in events]
        require(event_seq == sorted(set(event_seq))
                and all(type(seq) is int and after < seq <= next_seq for seq in event_seq),
                "The run activity did not match its advertised cursor range.")
        if page["hasMore"]:
            require(next_seq > after, "The run activity cursor stopped advancing.")
        counts.update(event["type"] for event in events)
        after = next_seq
        require(run["phase"] in ("running", "succeeded"), "The smoke run did not finish successfully.")
        if run["phase"] == "succeeded" and run.get("workerActive") is False and not page["hasMore"]:
            return run, counts
        time.sleep(min(0.25, max(0, deadline - time.monotonic())))
    raise SmokeError("The smoke run did not finish within its time limit.")


def check_history(client, run_id, sha, report=None):
    history = client.json("/api/runs")["runs"]
    saved = [run for run in history if run.get("id") == run_id]
    require(len(saved) == 1 and saved[0].get("phase") == "succeeded" and saved[0].get("flowSha256") == sha,
            "History did not include exactly one completed run with the expected identity.")
    reopened = client.json(f"/api/runs/{saved[0]['id']}?after=0")["run"]
    require(reopened.get("id") == run_id and reopened.get("phase") == "succeeded"
            and reopened.get("flowSha256") == sha, "The saved run could not be reopened.")
    if report is not None:
        require(reopened.get("report") == report, "The reopened run did not return the saved report.")


def check_calculator(client, run_timeout):
    archive, sha = calculator_bundle()
    installed = client.json(
        "/api/flows/import", method="POST", body=archive, content_type="application/x-tar",
    )["flow"]
    require(installed.get("id") == DIRECT_ID and installed.get("sha256") == sha,
            "Bundle import did not return the checksum-pinned calculator flow.")
    listed = [flow for flow in client.json("/api/flows")["flows"] if flow.get("sha256") == sha]
    require(len(listed) == 1 and listed[0].get("id") == DIRECT_ID, "The installed calculator was missing from the library.")
    inspection = client.json(f"/api/flows/{sha}/inspect")
    tasks = inspection["registry"]["tasks"]
    require(inspection.get("valid") is True and inspection.get("packageId") == sha
            and any(plan.get("id") == DIRECT_ID and plan.get("entryStep") == "calculate" for plan in inspection["plans"])
            and tasks["add"].get("tools") == ["local.calculate"]
            and tasks["check-result"].get("tools") == ["local.assert_equal"],
            "Inspection did not show the installed calculator steps and grants.")
    readiness = client.json(f"/api/flows/{sha}")
    require(readiness.get("canStart") is True and readiness.get("models") == [],
            "The calculator was not ready without model downloads.")
    run_id = start(client, sha, "", check_repeat=True)
    _, events = wait_for_run(client, run_id, sha, min(30, run_timeout))
    # The second direct task compares the saved calculator result against 80.09.
    # Success requires both artifacts without exposing their payloads over HTTP.
    require(events["direct_tool_step"] == 2 and events["artifact_written"] >= 2
            and events["model_call"] == 0, "The calculator proof did not record both direct tasks and artifacts.")
    check_history(client, run_id, sha)


def check_native_summary(client, model_timeout, run_timeout):
    flows = client.json("/api/flows")["flows"]
    matches = [flow for flow in flows if flow.get("id") == "text-summary" and flow.get("title") == "Summarize text"]
    require(len(matches) == 1, "Expected exactly one installed Summarize text starter.")
    flow = matches[0]
    sha = flow.get("sha256")
    require(isinstance(sha, str) and re.fullmatch(r"[a-f0-9]{64}", sha), "The summary starter lacks a valid checksum.")
    pins = flow["setup"]["models"]
    require(isinstance(pins, dict) and pins, "The summary starter does not declare pinned model files.")

    path = f"/api/flows/{sha}"
    deadline = time.monotonic() + model_timeout
    requested = set()
    while time.monotonic() < deadline:
        readiness = client.json(path, timeout=min(HTTP_TIMEOUT, max(0.1, deadline - time.monotonic())))
        models = readiness["models"]
        require(len(models) == len(pins) and {model["slot"] for model in models} == pins.keys(),
                "Readiness did not include all pinned summary models.")
        for model in models:
            slot = model["slot"]
            status = model["status"]
            require(model.get("sha256") == pins[slot].get("sha256") and model.get("sizeBytes") == pins[slot].get("sizeBytes"),
                    "Model readiness did not match the installed model pins.")
            if status in ("missing", "paused", "error") and slot not in requested:
                # Request each transfer at most once in this invocation.
                client.json(
                    f"{path}/models/{quote(slot, safe='')}/download", method="POST",
                    timeout=min(HTTP_TIMEOUT, max(0.1, deadline - time.monotonic())),
                )
                requested.add(slot)
            else:
                require(status in ("ready", "downloading"), "The pinned summary model download stopped before it was ready.")
        if readiness.get("canStart") is True and all(model["status"] == "ready" for model in models):
            break
        time.sleep(min(1, max(0, deadline - time.monotonic())))
    else:
        raise SmokeError("The summary starter did not become ready within its model time limit.")

    run_id = start(client, sha, SUMMARY_INPUT)
    run, events = wait_for_run(client, run_id, sha, run_timeout)
    report = run.get("report")
    require(events["model_call"] >= 1 and isinstance(report, str) and report.strip(),
            "The native summary did not produce a saved report.")
    check_history(client, run_id, sha, report)


def smoke(client, args, passphrase):
    check_health_and_ui(client, args.expect_backend)
    check_public_guards(client, args.expect_backend)
    unlock(client, passphrase)
    try:
        require(client.json("/api/bootstrap").get("needsSetup") is False, "The vault did not retain its setup state.")
        error_code(client.json("/api/flows", origin="https://outside.example.invalid", status=403), "ORIGIN_DENIED")
        check_calculator(client, args.run_timeout)
        if args.real_model:
            check_native_summary(client, args.model_timeout, args.run_timeout)
    except BaseException:
        with suppress(SmokeError):
            client.json("/api/session", method="DELETE")
        raise

    client.json("/api/session", method="DELETE")
    error_code(client.json("/api/runs", status=401), "SESSION_REQUIRED")
    client.token = ""
    print(f"Desktop {args.expect_backend} smoke passed. Verified built assets, HTTP guards, import, inspection, idempotent Start, exact calculation and saved history.")
    if args.real_model:
        print("The llama starter produced a nonempty report and returned it again from the saved run.")


def seconds(value):
    try:
        number = float(value)
    except ValueError:
        raise argparse.ArgumentTypeError("Timeout must be a positive number of seconds.") from None
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("Timeout must be a positive number of seconds.")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Configured desktop origin")
    parser.add_argument("--passphrase", help="Disposable vault passphrase. Prefer TROLLM_DESKTOP_TEST_PASSPHRASE.")
    parser.add_argument("--expect-backend", required=True, choices=("llama", "stub"))
    parser.add_argument("--real-model", action="store_true", help="Explicitly allow download and execution of the llama summary starter")
    parser.add_argument("--model-timeout", type=seconds, default=900, help="Seconds to wait for model readiness (default 900)")
    parser.add_argument("--run-timeout", type=seconds, default=180, help="Seconds to wait for a run (default 180)")
    args = parser.parse_args()
    if args.real_model and args.expect_backend != "llama":
        parser.error("--real-model requires --expect-backend llama")
    passphrase = args.passphrase or os.environ.get("TROLLM_DESKTOP_TEST_PASSPHRASE")
    if not passphrase:
        parser.error("Set TROLLM_DESKTOP_TEST_PASSPHRASE or provide --passphrase")

    try:
        smoke(DesktopClient(args.url), args, passphrase)
    except SmokeError as error:
        print(f"Desktop smoke failed. {error}", file=sys.stderr)
        return 1
    except (AttributeError, IndexError, KeyError, TypeError, ValueError):
        print("Desktop smoke failed. The service returned an unexpected response shape.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
