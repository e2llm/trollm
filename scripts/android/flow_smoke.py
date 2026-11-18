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
MAX_DETAILS_PER_PAGE = 20


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


def matching_control(root, text, package=PACKAGE, description=False):
    """A label is actionable only on an enabled clickable node or ancestor."""
    return next(controls(root, lambda label: label == text, package, description), None)


def first_control(root, labels, *, description=None):
    named = matching_control(root, description, description=True) if description is not None else None
    if named is not None:
        return named
    return next((node for label in labels if (node := matching_control(root, label)) is not None), None)


def controls(root, matches, package=PACKAGE, description=False):
    parents = {child: parent for parent in root.iter() for child in parent}
    packages = (package,) if isinstance(package, str) else package
    attribute = "content-desc" if description else "text"
    seen = set()
    for node in root.iter("node"):
        if (node.get("package") not in packages or not matches(node.get(attribute, ""))
                or node.get("enabled") != "true"):
            continue
        current = node
        while current is not None:
            if current.get("enabled") == "false":
                break
            if current.get("package") in packages and current.get("clickable") == "true":
                if current.get("enabled") == "true" and current not in seen:
                    seen.add(current)
                    yield current
                break
            current = parents.get(current)


def text_lines(root):
    return {line for node in root.iter("node") if node.get("package") == PACKAGE
            for line in node.get("text", "").splitlines()}


def wait_ui(capture, predicate, label):
    # A bounded number of captures; each subprocess has its own deadline.
    for _ in range(4):
        found = predicate(capture())
        if found is not None and found is not False:
            return found
        time.sleep(0.5)
    raise FlowSmokeError(f"Smoke-test UI did not reach {label}")


def scroll_find(adb, capture, predicate):
    """Search toward the top, then bottom, using only fresh observed bounds."""
    root = capture()
    found = predicate(root)
    if found is not None and found is not False:
        return found
    for direction in ("down", "up"):
        for _ in range(MAX_FLOW_LIST_SWIPES):
            if not app_scrollers(root):
                return None
            previous = ET.tostring(root)
            swipe_list(adb, root, direction=direction)
            root = capture()
            found = predicate(root)
            if found is not None and found is not False:
                return found
            if ET.tostring(root) == previous:
                break
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
        texts = {line for child in node.iter("node") for line in child.get("text", "").splitlines()}
        if flow_id in texts and (f"v1 · sha256 {sha256[:12]}…" in texts or "Version 1" in texts):
            matches.append(node)
    if len(matches) > 1:
        raise FlowSmokeError("Fixture flow is ambiguous in the visible UI")
    return matches[0] if matches else None


def app_scrollers(root):
    scrollers = [node for node in root.iter("node")
                 if node.get("package") == PACKAGE and node.get("enabled") == "true"
                 and node.get("scrollable") == "true"]
    nested = {descendant for node in scrollers for descendant in node.iter("node") if descendant is not node}
    return [node for node in scrollers if node not in nested]


def swipe_list(adb, root, *, direction="up"):
    scrollers = app_scrollers(root)
    if len(scrollers) != 1:
        raise FlowSmokeError("App list has no unique enabled scroller")
    scroller = scrollers[0]
    x, _ = center(scroller)  # Reject malformed or empty observed bounds.
    _, top, _, bottom = map(int, re.findall(r"\d+", scroller.get("bounds")))
    start_y = top + (bottom - top) * 3 // 4
    end_y = top + (bottom - top) // 4
    if start_y <= end_y:
        raise FlowSmokeError("App list bounds cannot contain an upward swipe")
    if direction == "down":
        start_y, end_y = end_y, start_y
    adb("shell", "input", "swipe", x, str(start_y), x, str(end_y), "350")


def open_list_card(adb, capture, root, find_card, label, *, disclose=False):
    """Search from the current list position with fresh UI/bounds at every step."""
    for attempt in range(MAX_FLOW_LIST_SWIPES + 1):
        for disclosure in range(MAX_DETAILS_PER_PAGE + 1):
            node = find_card(root)
            if node is not None:
                adb("shell", "input", "tap", *center(node))
                return node
            details = matching_control(root, "Technical details") if disclose else None
            if details is None:
                break
            if disclosure == MAX_DETAILS_PER_PAGE:
                raise FlowSmokeError("Too many technical disclosures on one smoke-test page")
            adb("shell", "input", "tap", *center(details))
            root = capture()
        if attempt == MAX_FLOW_LIST_SWIPES:
            raise FlowSmokeError(f"{label} identity not found after {MAX_FLOW_LIST_SWIPES} list swipes")
        swipe_list(adb, root)
        root = capture()


def dialog_scope(root, dismiss, marker):
    """Locate the nearest branch containing the dialog's dismiss control and text."""
    parents = {child: parent for parent in root.iter() for child in parent}
    candidates = set()
    for control in controls(root, lambda label: label == dismiss):
        branch = parents.get(control)
        while branch is not None:
            # A dump can include the library window behind the dialog.
            if any(node.get("content-desc", "").startswith("Details for ")
                   or node.get("content-desc") == "Manage flows" for node in branch.iter("node")):
                break
            if any(marker(line) for line in text_lines(branch)):
                candidates.add(branch)
                break
            branch = parents.get(branch)
    if len(candidates) > 1:
        raise FlowSmokeError("Smoke-test dialog is ambiguous")
    return next(iter(candidates), None)


def open_library_details(adb, capture, root, title, expected, action):
    def shown_details(label):
        return label == f"Details for {title}" if title is not None else label.startswith("Details for ")

    def library(root):
        if matching_control(root, "Manage flows", description=True) is None:
            return None
        if {"Loading flows…", "Loading run history…"} & text_lines(root):
            return None
        return root

    def details_dialog(root):
        return dialog_scope(root, "Done", lambda line: line.startswith(("Flow SHA-256: ", "Run ID: ")))

    def open_on_page(root):
        for index in range(MAX_DETAILS_PER_PAGE + 1):
            details = list(controls(root, shown_details, description=True))
            if index >= len(details):
                return root
            if index == MAX_DETAILS_PER_PAGE:
                raise FlowSmokeError("Too many library details controls on one smoke-test page")
            adb("shell", "input", "tap", *center(details[index]))
            dialog = wait_ui(capture, details_dialog, "library identity details")
            if expected in text_lines(dialog):
                buttons = list(controls(dialog, lambda label: label == action))
                if len(buttons) != 1:
                    raise FlowSmokeError("Verified library identity has no unique enabled action")
                adb("shell", "input", "tap", *center(buttons[0]))
                return None
            done = matching_control(dialog, "Done")
            adb("shell", "input", "tap", *center(done))
            root = wait_ui(capture, library, "flow library")
        raise FlowSmokeError("Library details search exhausted")

    if library(root) is None:
        root = wait_ui(capture, library, "flow library")
    root = open_on_page(root)
    for direction in ("down", "up"):
        for _ in range(MAX_FLOW_LIST_SWIPES):
            if root is None:
                return
            if not app_scrollers(root):
                break
            previous = ET.tostring(root)
            swipe_list(adb, root, direction=direction)
            root = wait_ui(capture, library, "flow library")
            if ET.tostring(root) == previous:
                break
            root = open_on_page(root)
    if root is not None:
        raise FlowSmokeError("Expected full library identity was not found in bounded list swipes")


def open_fixture_card(adb, capture, flow_id, sha256):
    root = capture()
    if matching_control(root, "Manage flows", description=True) is not None:
        open_library_details(adb, capture, root, flow_id, f"Flow SHA-256: {sha256}", "Open flow")
        return True
    open_list_card(adb, capture, root, lambda root: flow_node(root, flow_id, sha256), "Fixture")
    return False


def verify_identity(adb, capture, *expected):
    # New disclosures keep their label when opened. Toggle only once, and use
    # freshly observed controls after the full identity has appeared.
    opened = False
    for page in range(MAX_FLOW_LIST_SWIPES + 1):
        root = capture()
        details = matching_control(root, "Technical details")
        if not opened and details is not None and not text_lines(root).intersection(expected):
            adb("shell", "input", "tap", *center(details))
            root = capture()
            opened = True
        if text_lines(root).intersection(expected):
            hide = matching_control(root, "Hide technical details")
            if hide is None and opened:
                hide = matching_control(root, "Technical details")
            if hide is not None:
                adb("shell", "input", "tap", *center(hide))
            return
        if page == MAX_FLOW_LIST_SWIPES:
            break
        try:
            swipe_list(adb, root)
        except FlowSmokeError as failure:
            raise FlowSmokeError("Expected full identity is unavailable in technical details") from failure
    raise FlowSmokeError("Expected full identity was not found in bounded screen swipes")


def history_node(root, run_id):
    matches = [node for node in root.iter("node")
               if node.get("package") == PACKAGE and node.get("clickable") == "true"
               and node.get("enabled") == "true"
               and any(run_id == child.get("text") or f"Run ID: {run_id}" in child.get("text", "").splitlines()
                       for child in node.iter("node"))]
    if len(matches) > 1:
        raise FlowSmokeError("Run identity is ambiguous in the visible history")
    return matches[0] if matches else None


def open_history_card(adb, capture, run_id, title=None):
    if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id) is None:
        raise FlowSmokeError("History target has an invalid run identity")
    root = capture()
    if matching_control(root, "Manage flows", description=True) is not None:
        open_library_details(adb, capture, root, title, f"Run ID: {run_id}", "View run")
        return
    open_list_card(adb, capture, root, lambda root: history_node(root, run_id), "Run", disclose=True)


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
        return wait_ui(capture, predicate, label)

    def wait_visible(predicate, label):
        try:
            return wait_for(predicate, label)
        except FlowSmokeError:
            found = scroll_find(adb, capture, predicate)
            if found is not None and found is not False:
                return found
            raise FlowSmokeError(f"Smoke-test UI did not reach {label} after bounded scrolling")

    def tap(text, package=PACKAGE, description=False):
        labels = (text,) if isinstance(text, str) else text
        node = wait_visible(lambda root: next((found for label in labels
                        if (found := matching_control(root, label, package, description)) is not None), None), "expected control")
        adb("shell", "input", "tap", *center(node))

    def wait_text(expected, *, exact=False):
        def find(root):
            return next((node for node in root.iter("node")
                         if node.get("package") == PACKAGE
                         and (node.get("text") == expected if exact else expected in node.get("text", ""))), None)
        return wait_visible(find, "expected result")

    def wait_run_state(title, message, legacy):
        def matches(root):
            lines = text_lines(root)
            return legacy in lines or {title, message} <= lines
        wait_visible(matches, "expected run state")

    def wait_paused():
        wait_run_state("Paused", fixture.PAUSE_REASON, f"Paused: {fixture.PAUSE_REASON}")

    def go_up():
        node = wait_visible(lambda root: first_control(root, ("‹ Back", "Back"), description="Navigate up"),
                            "navigate up")
        adb("shell", "input", "tap", *center(node))

    def manage_flows(root):
        return matching_control(root, "Manage flows", description=True)

    def verify_run(run_id):
        verify_identity(adb, capture, f"Run ID · {run_id}", f"Run ID: {run_id}", run_id)

    def confirm_cancel(root):
        dialog = dialog_scope(root, "Keep run", lambda line: line == "Cancel this run?")
        if dialog is None:
            return None
        buttons = list(controls(dialog, lambda label: label == "Cancel run"))
        if len(buttons) > 1:
            raise FlowSmokeError("Run cancellation confirmation is ambiguous")
        return buttons[0] if buttons else None

    def install(local_name, expected):
        # A unique filename avoids selecting an unrelated/stale Downloads item.
        remote_name = f"trollm-smoke-{uuid.uuid4().hex}.tar"
        remote = f"/sdcard/Download/{remote_name}"
        remote_paths.append(remote)
        adb("push", str(fixture.OUTPUT_DIR / local_name), remote)
        if manage_flows(capture()) is not None:
            tap("Manage flows", description=True)
            tap("Import flow file")
        else:
            tap(("Install flow", "Install bundle"))
        tap("Choose file")
        root = capture()
        # The system document picker may open Recent or a previously used folder.
        if matching_node(root, remote_name, DOCUMENTS) is None:
            tap("Show roots", DOCUMENTS, description=True)
            tap("Downloads", DOCUMENTS)
        tap(remote_name, DOCUMENTS)
        if expected.startswith("Installed: "):
            flow_id = expected.removeprefix("Installed: ")
            install_labels = {expected, f"{expected} v1", f"{flow_id} · Version 1"}
            wait_visible(lambda root: bool(text_lines(root) & install_labels), "installed fixture identity")
        else:
            wait_text(expected)
        go_up()

    def private_entries(subdir):
        return set(adb("shell", "run-as", PACKAGE, "ls", f"files/trollm/{subdir}").splitlines())

    def open_fixture(paused=False):
        flow_id = fixture.PAUSED_FLOW_ID if paused else fixture.FLOW_ID
        digest = hashlib.sha256(fixture.fixture_files(paused=paused)[f"{flow_id}.flow.yaml"]).hexdigest()
        if not open_fixture_card(adb, capture, flow_id, digest):
            verify_identity(adb, capture, f"Bundle SHA-256: {digest}", f"Flow SHA-256 · {digest}")

    try:
        # A previous run may have restored the run/install screen.
        def home(root):
            return first_control(root, ("Install flow", "Install bundle"), description="Manage flows")
        if scroll_find(adb, capture, home) is None:
            go_up()
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
        wait_visible(lambda root: next((found for label in ("Completed", "Succeeded")
                         if (found := matching_node(root, label)) is not None), None), "completed run")
        wait_text(fixture.EXPECTED_REPORT)
        checks.append("stub-run-final-report")
        new_runs = private_entries("runs") - before_runs
        if len(new_runs) != 1:
            raise FlowSmokeError("Expected exactly one new smoke run")
        run_id = new_runs.pop()
        if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id) is None:
            raise FlowSmokeError("Created smoke run has an invalid identity")
        print("PASS: installed fixture completed through JNI with expected final report")
        go_up()

        # Search freshly after Back: installed flows may precede the history.
        open_history_card(adb, capture, run_id, fixture.FLOW_ID)
        verify_run(run_id)
        wait_text(fixture.EXPECTED_REPORT)
        checks.append("history-reopen")
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        verify_run(run_id)
        wait_text(fixture.EXPECTED_REPORT)
        wait_visible(lambda root: next((found for label in ("Completed", "Succeeded")
                         if (found := matching_node(root, label)) is not None), None), "completed run")
        checks.append("completed-run-cold-recovery")
        print("PASS: run history and completed-run recovery after process death")

        go_up()
        install("android-smoke-paused.tar", "Installed: android-smoke-paused")
        open_fixture(paused=True)
        before_runs = private_entries("runs")
        tap("Start run")
        wait_paused()
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

        initial = events()
        initial_types = [event.get("type") for event in initial]
        if (initial_types[-1:] != ["paused"] or "resumed" in initial_types
                or initial_types.count("model_call") != 1):
            raise FlowSmokeError("Initial paused fixture has missing or unexpected execution events")

        adb("shell", "input", "keyevent", "KEYCODE_HOME")
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        wait_paused()
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", f"{PACKAGE}/.MainActivity", timeout=30)
        verify_run(paused_id)
        wait_run_state("Interrupted", "Restore progress using the saved run.",
                       "Interrupted — tap Resume to restore saved progress. Models load when needed.")
        resume = ("Resume saved run", "Resume")
        wait_visible(lambda root: first_control(root, resume), "enabled explicit Resume")
        cold = events()
        if cold != initial:
            raise FlowSmokeError("Cold reopen changed the journal before explicit Resume")
        cold_types = [event.get("type") for event in cold]
        checks.append("paused-run-cold-recovery")
        before_resume = cold_types.count("resumed")
        before_models = cold_types.count("model_call")
        tap(resume)

        def resumed(_root):
            types = [event.get("type") for event in events()]
            return (types.count("resumed") == before_resume + 1
                    and types.count("model_call") == before_models + 1 and types[-1:] == ["paused"])

        wait_for(resumed, "resumed execution and repeated pause")
        wait_paused()
        checks.append("resume-reruns-task")
        def cancel_control(root):
            menu = matching_control(root, "Run actions", description=True)
            if menu is not None:
                return menu, True
            direct = first_control(root, ("Cancel run", "Cancel"))
            return (direct, False) if direct is not None else None

        node, through_menu = wait_visible(cancel_control, "cancel action")
        adb("shell", "input", "tap", *center(node))
        if through_menu:
            tap("Cancel run")
        def cancellation(root):
            confirmation = confirm_cancel(root)
            if confirmation is not None:
                return confirmation
            return matching_node(root, "Cancelled") if not through_menu else None

        confirmed = wait_for(cancellation, "enabled confirmation or cancellation")
        if confirmed.get("text") != "Cancelled":
            adb("shell", "input", "tap", *center(confirmed))
        wait_text("Cancelled", exact=True)
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
