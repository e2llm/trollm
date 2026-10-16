"""Local UI journey using tiny deterministic fixtures; no server or model access."""

import hashlib
import json
import re
import time
import uuid
import xml.etree.ElementTree as ET

import make_smoke_bundle as fixture

PACKAGE = "com.trollm.app"
DOCUMENTS = ("com.google.android.documentsui", "com.android.documentsui")
MAX_FLOW_LIST_SWIPES = 20


class FlowSmokeError(Exception):
    pass


def parse_ui(output):
    start, end = output.find("<?xml"), output.rfind("</hierarchy>")
    if start < 0 or end < 0:
        raise FlowSmokeError("Could not capture the smoke-test UI")
    return ET.fromstring(output[start:end + len("</hierarchy>")])


def matching_node(root, text, package=PACKAGE, description=False):
    attribute = "content-desc" if description else "text"
    packages = (package,) if isinstance(package, str) else package
    for node in root.iter("node"):
        if node.get("package") in packages and node.get(attribute) == text and node.get("enabled") == "true":
            return node
    return None


def center(node):
    bounds = re.fullmatch(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", node.get("bounds", ""))
    if bounds is None:
        raise FlowSmokeError("Smoke-test control has no valid bounds")
    left, top, right, bottom = map(int, bounds.groups())
    if left >= right or top >= bottom:
        raise FlowSmokeError("Smoke-test control has empty bounds")
    return str((left + right) // 2), str((top + bottom) // 2)


def flow_node(root, flow_id, sha256):
    matches = []
    for node in root.iter("node"):
        if node.get("package") != PACKAGE or node.get("clickable") != "true" or node.get("enabled") != "true":
            continue
        texts = {child.get("text") for child in node.iter("node")}
        if flow_id in texts and f"v1 · sha256 {sha256[:12]}…" in texts:
            matches.append(node)
    if len(matches) > 1:
        raise FlowSmokeError("Fixture flow is ambiguous in the visible UI")
    return matches[0] if matches else None


def open_list_card(adb, capture, find_card, label):
    """Search from the current list position with fresh UI/bounds at every step."""
    for attempt in range(MAX_FLOW_LIST_SWIPES + 1):
        root = capture()
        node = find_card(root)
        if node is not None:
            adb("shell", "input", "tap", *center(node))
            return
        if attempt == MAX_FLOW_LIST_SWIPES:
            raise FlowSmokeError(f"{label} identity not found after {MAX_FLOW_LIST_SWIPES} list swipes")
        scrollers = [node for node in root.iter("node")
                     if node.get("package") == PACKAGE and node.get("enabled") == "true"
                     and node.get("scrollable") == "true"]
        if len(scrollers) != 1:
            raise FlowSmokeError("App list has no unique enabled scroller")
        scroller = scrollers[0]
        x, _ = center(scroller)  # Reject malformed or empty observed bounds.
        _, top, _, bottom = map(int, re.findall(r"\d+", scroller.get("bounds")))
        start_y = top + (bottom - top) * 3 // 4
        end_y = top + (bottom - top) // 4
        if start_y <= end_y:
            raise FlowSmokeError("App list bounds cannot contain an upward swipe")
        adb("shell", "input", "swipe", x, str(start_y), x, str(end_y), "350")


def open_fixture_card(adb, capture, flow_id, sha256):
    """Match both fixture name and flow SHA, including after an import return."""
    open_list_card(adb, capture, lambda root: flow_node(root, flow_id, sha256), "Fixture")


def history_node(root, run_id):
    matches = [node for node in root.iter("node")
               if node.get("package") == PACKAGE and node.get("clickable") == "true"
               and node.get("enabled") == "true"
               and any(child.get("text") == run_id for child in node.iter("node"))]
    if len(matches) > 1:
        raise FlowSmokeError("Run identity is ambiguous in the visible history")
    return matches[0] if matches else None


def open_history_card(adb, capture, run_id):
    if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id) is None:
        raise FlowSmokeError("History target has an invalid run identity")
    open_list_card(adb, capture, lambda root: history_node(root, run_id), "Run")


def exercise_flow(adb):
    """Use the app file picker and run UI; preserve data and other applications."""
    fixture.main()
    checks = []
    remote_paths = []

    def capture():
        for attempt in range(2):
            try:
                return parse_ui(adb("exec-out", "uiautomator", "dump", "/dev/tty", timeout=30))
            except FlowSmokeError:
                if attempt:
                    raise
                # Opening/closing DocumentsUI can briefly expose no window.
                time.sleep(0.5)

    def wait_for(predicate, label):
        # A bounded number of captures; each subprocess has its own deadline.
        for _ in range(4):
            root = capture()
            found = predicate(root)
            if found is not None and found is not False:
                return found
            time.sleep(0.5)
        raise FlowSmokeError(f"Smoke-test UI did not reach {label}")

    def tap(text, package=PACKAGE, description=False):
        node = wait_for(lambda root: matching_node(root, text, package, description), "expected control")
        adb("shell", "input", "tap", *center(node))

    def wait_text(expected):
        def find(root):
            return next((node for node in root.iter("node")
                         if node.get("package") == PACKAGE and expected in node.get("text", "")), None)
        return wait_for(find, "expected result")

    def install(local_name, expected):
        # A unique filename avoids selecting an unrelated/stale Downloads item.
        remote_name = f"trollm-smoke-{uuid.uuid4().hex}.tar"
        remote = f"/sdcard/Download/{remote_name}"
        remote_paths.append(remote)
        adb("push", str(fixture.OUTPUT_DIR / local_name), remote)
        tap("Install bundle")
        tap("Choose file")
        root = capture()
        # The system document picker may open Recent or a previously used folder.
        if matching_node(root, remote_name, DOCUMENTS) is None:
            tap("Show roots", DOCUMENTS, description=True)
            tap("Downloads", DOCUMENTS)
        tap(remote_name, DOCUMENTS)
        wait_text(expected)
        tap("Back")

    def private_entries(subdir):
        return set(adb("shell", "run-as", PACKAGE, "ls", f"files/trollm/{subdir}").splitlines())

    def open_fixture(paused=False):
        flow_id = fixture.PAUSED_FLOW_ID if paused else fixture.FLOW_ID
        digest = hashlib.sha256(fixture.fixture_files(paused=paused)[f"{flow_id}.flow.yaml"]).hexdigest()
        open_fixture_card(adb, capture, flow_id, digest)

    try:
        # A previous run may have restored the run/install screen.
        root = capture()
        if matching_node(root, "Install bundle") is None:
            tap("Back")
        before_flows = private_entries("flows")
        install(f"{fixture.FLOW_ID}-bad-hash.tar", "sha256 mismatch")
        if private_entries("flows") != before_flows:
            raise FlowSmokeError("Rejected bundle changed the installed flow set")
        checks.append("reject-corrupt-bundle")
        print("PASS: corrupt bundle rejected through Android file picker")
        install(f"{fixture.FLOW_ID}.tar", f"Installed: {fixture.FLOW_ID}")
        checks.append("install-valid-bundle")
        open_fixture()
        before_runs = private_entries("runs")
        tap("Start run")
        wait_text("succeeded")
        wait_text(fixture.EXPECTED_REPORT)
        checks.append("stub-run-final-report")
        new_runs = private_entries("runs") - before_runs
        if len(new_runs) != 1:
            raise FlowSmokeError("Expected exactly one new smoke run")
        run_id = new_runs.pop()
        if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id) is None:
            raise FlowSmokeError("Created smoke run has an invalid identity")
        print("PASS: installed fixture completed through JNI with expected final report")
        tap("Back")

        # Search freshly after Back: installed flows may precede the history.
        open_history_card(adb, capture, run_id)
        wait_text(run_id)
        wait_text(fixture.EXPECTED_REPORT)
        checks.append("history-reopen")
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        wait_text(run_id)
        wait_text(fixture.EXPECTED_REPORT)
        wait_text("succeeded")
        checks.append("completed-run-cold-recovery")
        print("PASS: run history and completed-run recovery after process death")

        tap("Back")
        install("android-smoke-paused.tar", "Installed: android-smoke-paused")
        open_fixture(paused=True)
        before_runs = private_entries("runs")
        tap("Start run")
        wait_text("paused: needs-attention")
        new_runs = private_entries("runs") - before_runs
        if len(new_runs) != 1:
            raise FlowSmokeError("Expected exactly one new paused smoke run")
        paused_id = new_runs.pop()
        if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", paused_id) is None:
            raise FlowSmokeError("Created paused run has an invalid identity")
        checks.append("plan-driven-pause")

        # Inspect only this new, server-free fixture's bounded event log. It
        # distinguishes a real resumed execution from the previous paused UI.
        def events():
            output = adb("shell", "run-as", PACKAGE, "head", "-c", "32768",
                         f"files/trollm/runs/{paused_id}/events.jsonl")
            try:
                return [json.loads(line) for line in output.splitlines() if line.strip()]
            except json.JSONDecodeError:
                return []  # A worker may be appending; the next bounded probe retries.

        adb("shell", "input", "keyevent", "KEYCODE_HOME")
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        wait_text("paused: needs-attention")
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        wait_text(paused_id)
        wait_text("paused: needs-attention")
        checks.append("paused-run-cold-recovery")
        before_resume = sum(event.get("type") == "resumed" for event in events())
        tap("Resume")

        def resumed(_root):
            types = [event.get("type") for event in events()]
            return (types.count("resumed") == before_resume + 1
                    and types.count("model_call") >= 2 and types[-1:] == ["paused"])

        wait_for(resumed, "resumed execution and repeated pause")
        wait_text("paused: needs-attention")
        checks.append("resume-reruns-task")
        tap("Cancel")
        wait_text("failed: cancelled")
        terminal = events()
        if not terminal or terminal[-1].get("type") != "run_ended" or terminal[-1].get("outcome") != "cancelled":
            raise FlowSmokeError("Cancelled fixture did not persist its terminal outcome")
        checks.append("cancel-paused-run")
        print("PASS: paused-run cold recovery, actual resumed execution and cancellation")
        return checks
    finally:
        # Remove only the tiny unique files created by this invocation.
        for path in remote_paths:
            adb("shell", "rm", "-f", path)
