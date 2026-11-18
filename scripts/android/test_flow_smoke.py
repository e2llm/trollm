"""UI control selection guards; no Android device or SDK required."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import flow_smoke
import xml.etree.ElementTree as ET

from flow_smoke import (MAX_FLOW_LIST_SWIPES, FlowSmokeError, center, flow_node,
                        history_node, matching_node, open_fixture_card, open_history_card, parse_ui)


class FlowSmokeGuards(unittest.TestCase):
    def fixture_ui(self, *, visible=False, digest="a" * 64, scrollers=None):
        root = ET.Element("hierarchy")
        if scrollers is None:
            scrollers = [{"package": "com.trollm.app", "enabled": "true",
                          "scrollable": "true", "bounds": "[20,100][220,500]"}]
        for attributes in scrollers:
            ET.SubElement(root, "node", **attributes)
        if visible:
            card = ET.SubElement(root, "node", package="com.trollm.app", clickable="true",
                                 enabled="true", bounds="[40,200][200,260]")
            ET.SubElement(card, "node", text="android-smoke")
            ET.SubElement(card, "node", text=f"v1 · sha256 {digest[:12]}…")
        return root

    def open_fixture(self, roots, *, verify_legacy=False):
        events = []
        frames = iter(roots)

        def capture():
            events.append("capture")
            return next(frames)

        def adb(*parts):
            events.append(parts)

        def open_card():
            verified = open_fixture_card(adb, capture, "android-smoke", "a" * 64)
            if verify_legacy and not verified:
                flow_smoke.verify_identity(adb, capture, "Bundle SHA-256: " + "a" * 64)

        return events, open_card

    def test_visible_fixture_taps_without_swiping(self):
        events, open_card = self.open_fixture([self.fixture_ui(visible=True, scrollers=[])])
        open_card()
        self.assertEqual(events, ["capture", ("shell", "input", "tap", "120", "230")])

    def test_hidden_fixture_uses_observed_bounds_then_fresh_card(self):
        events, open_card = self.open_fixture([self.fixture_ui(), self.fixture_ui(visible=True)])
        open_card()
        self.assertEqual(events, ["capture",
                                 ("shell", "input", "swipe", "120", "400", "120", "200", "350"),
                                 "capture", ("shell", "input", "tap", "120", "230")])

    def test_fixture_beyond_three_swipes_remains_reachable_with_long_history(self):
        roots = []
        for page in range(8):
            root = self.fixture_ui()
            ET.SubElement(root, "node", package="com.trollm.app", text=f"older-run-page-{page}")
            roots.append(root)
        roots.append(self.fixture_ui(visible=True))
        events, open_card = self.open_fixture(roots)
        open_card()
        self.assertEqual(sum(isinstance(event, tuple) and event[2] == "swipe"
                             for event in events), 8)
        self.assertEqual(events[-1], ("shell", "input", "tap", "120", "230"))

    def test_history_beyond_installed_flows_uses_exact_run_identity(self):
        target = "12345678-1234-1234-1234-123456789abc"
        wrong = "12345678-1234-1234-1234-123456789abd"
        roots = []
        for page in range(6):
            root = self.fixture_ui(visible=True)
            card = ET.SubElement(root, "node", package="com.trollm.app", clickable="true",
                                 enabled="true", bounds="[40,200][200,260]")
            ET.SubElement(card, "node", text=wrong)
            roots.append(root)
        visible = self.fixture_ui()
        card = ET.SubElement(visible, "node", package="com.trollm.app", clickable="true",
                             enabled="true", bounds="[40,300][200,360]")
        ET.SubElement(card, "node", text=target)
        roots.append(visible)
        frames = iter(roots)
        events = []
        open_history_card(lambda *args: events.append(args), lambda: next(frames), target)
        self.assertEqual(len([event for event in events if event[2] == "swipe"]), 6)
        self.assertEqual(events[-1], ("shell", "input", "tap", "120", "330"))
        foreign = self.fixture_ui()
        card.set("package", "unrelated.app")
        foreign.append(card)
        self.assertIsNone(history_node(foreign, target))

    def test_each_import_return_search_starts_from_fresh_list_ui(self):
        events, open_card = self.open_fixture([self.fixture_ui(), self.fixture_ui(visible=True)])
        open_card()
        events2, open_again = self.open_fixture([self.fixture_ui(visible=True)])
        open_again()
        self.assertEqual(events2, ["capture", ("shell", "input", "tap", "120", "230")])

    def test_missing_ambiguous_foreign_and_disabled_scrollers_are_rejected(self):
        valid = {"package": "com.trollm.app", "enabled": "true",
                 "scrollable": "true", "bounds": "[20,100][220,500]"}
        for scrollers in ([], [valid, valid], [{**valid, "package": "unrelated.app"}],
                          [{**valid, "enabled": "false"}]):
            with self.subTest(scrollers=scrollers):
                events, open_card = self.open_fixture([self.fixture_ui(scrollers=scrollers)])
                with self.assertRaises(FlowSmokeError):
                    open_card()
                self.assertEqual(events, ["capture"])

    def test_scroller_bounds_must_allow_an_upward_swipe(self):
        for bounds in ("SECRET", "", "[20,100][20,500]", "[220,100][20,500]",
                       "[20,500][220,100]", "[20,100][220,100]", "[20,100][220,101]"):
            with self.subTest(bounds=bounds):
                root = self.fixture_ui(scrollers=[{"package": "com.trollm.app", "enabled": "true",
                                                  "scrollable": "true", "bounds": bounds}])
                events, open_card = self.open_fixture([root])
                with self.assertRaises(FlowSmokeError) as error:
                    open_card()
                self.assertNotIn("SECRET", str(error.exception))
                self.assertEqual(events, ["capture"])

    def test_search_is_bounded_without_tapping_a_wrong_hash(self):
        roots = [self.fixture_ui(visible=True, digest="b" * 64)
                 for _ in range(MAX_FLOW_LIST_SWIPES + 1)]
        events, open_card = self.open_fixture(roots)
        with self.assertRaises(FlowSmokeError):
            open_card()
        self.assertEqual(events.count("capture"), MAX_FLOW_LIST_SWIPES + 1)
        self.assertEqual(sum(isinstance(event, tuple) and event[2] == "swipe"
                             for event in events), MAX_FLOW_LIST_SWIPES)
        self.assertFalse(any(isinstance(event, tuple) and event[2] == "tap" for event in events))

    def test_fresh_ui_revalidates_scroller_bounds_and_fixture_ambiguity(self):
        second = self.fixture_ui(scrollers=[{"package": "com.trollm.app", "enabled": "true",
                                            "scrollable": "true", "bounds": "[30,200][230,600]"}])
        events, open_card = self.open_fixture([self.fixture_ui(), second, self.fixture_ui(visible=True)])
        open_card()
        self.assertEqual(events[3], ("shell", "input", "swipe", "130", "500", "130", "300", "350"))
        ambiguous = self.fixture_ui(visible=True)
        ambiguous.append(self.fixture_ui(visible=True)[-1])
        events, open_card = self.open_fixture([self.fixture_ui(), ambiguous])
        with self.assertRaisesRegex(FlowSmokeError, "ambiguous"):
            open_card()
        self.assertEqual(events[-1], "capture")

    def test_identical_labels_cannot_select_other_apps_or_disabled_controls(self):
        root = ET.fromstring('''<hierarchy>
          <node package="unrelated.app" text="Start run" enabled="true" />
          <node package="com.trollm.app" text="Start run" enabled="false" />
        </hierarchy>''')
        self.assertIsNone(matching_node(root, "Start run"))
        expected = ET.SubElement(root, "node", package="com.trollm.app",
                                 text="Start run", enabled="true", bounds="[20,30][80,70]")
        self.assertIs(matching_node(root, "Start run"), expected)
        self.assertEqual(center(expected), ("50", "50"))

    def test_invalid_or_empty_bounds_are_not_clicked(self):
        for bounds in ("", "[50,0][20,10]", "[0,0][0,10]", "SECRET"):
            with self.subTest(bounds=bounds), self.assertRaises(FlowSmokeError) as error:
                center(ET.Element("node", bounds=bounds))
            self.assertNotIn("SECRET", str(error.exception))

    def test_document_picker_description_uses_observed_control(self):
        root = parse_ui('''<?xml version="1.0"?><hierarchy>
          <node package="com.android.documentsui" content-desc="Show roots" enabled="true" />
        </hierarchy>UI hierarchy dumped to: /dev/tty''')
        node = matching_node(root, "Show roots", ("com.google.android.documentsui", "com.android.documentsui"), True)
        self.assertIsNotNone(node)
        with self.assertRaises(FlowSmokeError):
            parse_ui("no UI capture; SECRET")

    def test_flow_selection_requires_name_and_hash_on_the_same_card(self):
        root = ET.fromstring('''<hierarchy>
          <node package="com.trollm.app" clickable="true" enabled="true">
            <node text="android-smoke" /><node text="v1 · sha256 bbbbbbbbbbbb…" />
          </node>
          <node package="com.trollm.app" clickable="true" enabled="true">
            <node text="unrelated-flow" /><node text="v1 · sha256 aaaaaaaaaaaa…" />
          </node>
        </hierarchy>''')
        self.assertIsNone(flow_node(root, "android-smoke", "a" * 64))
        root[1][0].set("text", "android-smoke")
        self.assertIs(flow_node(root, "android-smoke", "a" * 64), root[1])

    def test_polished_fixture_candidate_still_requires_full_sha_after_open(self):
        root = self.fixture_ui(visible=True)
        root[-1][-1].set("text", "Version 1")
        details = ET.fromstring("""<hierarchy><node package="com.trollm.app"
            text="Technical details" enabled="true" clickable="true" bounds="[20,300][200,340]" /></hierarchy>""")
        verified = ET.Element("hierarchy")
        ET.SubElement(verified, "node", package=flow_smoke.PACKAGE,
                      text="Flow ID: android-smoke\nBundle SHA-256: " + "a" * 64)
        events, open_card = self.open_fixture([root, details, verified], verify_legacy=True)
        open_card()
        self.assertEqual([event for event in events if isinstance(event, tuple)],
                         [("shell", "input", "tap", "120", "230"),
                          ("shell", "input", "tap", "110", "320")])
        ambiguous = self.fixture_ui(visible=True)
        ambiguous[-1][-1].set("text", "Version 1")
        ambiguous.append(ambiguous[-1])
        events, open_card = self.open_fixture([ambiguous])
        with self.assertRaisesRegex(FlowSmokeError, "ambiguous"):
            open_card()
        self.assertEqual(events, ["capture"])

    def test_polished_identity_disclosure_can_be_below_fold(self):
        empty = self.fixture_ui()
        details = self.fixture_ui()
        ET.SubElement(details, "node", package=flow_smoke.PACKAGE, text="Technical details",
                      enabled="true", clickable="true", bounds="[20,300][200,340]")
        verified = self.fixture_ui()
        ET.SubElement(verified, "node", package=flow_smoke.PACKAGE, text="Run ID: exact-target")
        frames = iter([empty, details, verified])
        calls = []
        flow_smoke.verify_identity(lambda *args: calls.append(args), lambda: next(frames), "Run ID: exact-target")
        self.assertEqual(calls, [("shell", "input", "swipe", "120", "400", "120", "200", "350"),
                                 ("shell", "input", "tap", "110", "320")])

    def test_hidden_history_disclosure_search_is_bounded_and_never_guesses_title(self):
        target = "12345678-1234-1234-1234-123456789abc"
        root = self.fixture_ui()
        card = ET.SubElement(root, "node", package=flow_smoke.PACKAGE, clickable="true",
                             enabled="true", bounds="[20,200][200,400]")
        ET.SubElement(card, "node", text="Same title and date")
        ET.SubElement(card, "node", package=flow_smoke.PACKAGE, text="Technical details",
                      enabled="true", clickable="true", bounds="[20,300][200,340]")
        calls = []
        captures = []
        def capture():
            captures.append(True)
            return root
        with self.assertRaisesRegex(FlowSmokeError, "disclosures"):
            open_history_card(lambda *args: calls.append(args), capture, target)
        self.assertEqual(len(captures), flow_smoke.MAX_DETAILS_PER_PAGE + 1)
        self.assertEqual(len(calls), flow_smoke.MAX_DETAILS_PER_PAGE)
        self.assertTrue(all(call[-2:] == ("110", "320") for call in calls))


    def test_control_labels_require_clickable_parent_and_ignore_install_heading(self):
        root = ET.fromstring("""<hierarchy>
          <node package="com.trollm.app" text="Install flow" enabled="true" />
          <node package="com.trollm.app" enabled="true" clickable="true" bounds="[0,0][200,80]">
            <node package="com.trollm.app" text="‹ Back" enabled="true" />
          </node>
        </hierarchy>""")
        self.assertIsNone(flow_smoke.matching_control(root, "Install flow"))
        self.assertIs(flow_smoke.matching_control(root, "‹ Back"), root[1])

    def test_control_search_uses_both_directions_and_rejects_ambiguous_scrollers(self):
        middle = self.fixture_ui()
        top = self.fixture_ui()
        ET.SubElement(top, "node", package=flow_smoke.PACKAGE, text="At top")
        bottom = self.fixture_ui()
        expected = ET.SubElement(bottom, "node", package=flow_smoke.PACKAGE, text="Start run",
                                 clickable="true", enabled="true", bounds="[40,300][200,350]")
        frames = iter([middle, top, top, bottom])
        calls = []
        result = flow_smoke.scroll_find(lambda *args: calls.append(args), lambda: next(frames),
                                       lambda root: flow_smoke.matching_control(root, "Start run"))
        self.assertIs(result, expected)
        self.assertEqual(calls[0], ("shell", "input", "swipe", "120", "200", "120", "400", "350"))
        self.assertEqual(calls[-1], ("shell", "input", "swipe", "120", "400", "120", "200", "350"))
        ambiguous = self.fixture_ui()
        ambiguous.append(ambiguous[0])
        calls.clear()
        with self.assertRaisesRegex(FlowSmokeError, "unique"):
            flow_smoke.scroll_find(lambda *args: calls.append(args), lambda: ambiguous, lambda root: None)
        self.assertEqual(calls, [])

    def test_library_details_below_fold_take_only_the_verified_dialog_action(self):
        top = self.fixture_ui()
        ET.SubElement(top, "node", package=flow_smoke.PACKAGE, enabled="true", clickable="true",
                      **{"content-desc": "Manage flows"})
        below = flow_smoke.parse_ui('<?xml version="1.0"?>' + ET.tostring(top, encoding="unicode"))
        ET.SubElement(below, "node", package=flow_smoke.PACKAGE, enabled="true", clickable="true",
                      bounds="[240,300][280,340]", **{"content-desc": "Details for android-smoke"})
        dialog = ET.fromstring("""<hierarchy><node package="com.trollm.app">
          <node package="com.trollm.app" text="Done" enabled="true" clickable="true" bounds="[30,400][100,440]" />
          <node package="com.trollm.app" text="Open flow" enabled="true" clickable="true" bounds="[110,400][220,440]" />
        </node></hierarchy>""")
        ET.SubElement(dialog[0], "node", package=flow_smoke.PACKAGE, text="Flow SHA-256: " + "a" * 64)
        events, open_card = self.open_fixture([top, top, below, dialog])
        open_card()
        self.assertEqual([event for event in events if isinstance(event, tuple)], [
            ("shell", "input", "swipe", "120", "200", "120", "400", "350"),
            ("shell", "input", "swipe", "120", "400", "120", "200", "350"),
            ("shell", "input", "tap", "260", "320"),
            ("shell", "input", "tap", "165", "420"),
        ])

    def test_nested_event_list_does_not_compete_with_outer_screen_scroller(self):
        root = self.fixture_ui()
        ET.SubElement(root[0], "node", package=flow_smoke.PACKAGE, enabled="true",
                      scrollable="true", bounds="[30,300][220,500]")
        calls = []
        flow_smoke.swipe_list(lambda *args: calls.append(args), root)
        self.assertEqual(calls, [("shell", "input", "swipe", "120", "400", "120", "200", "350")])


class FakeFlowDevice:
    """Small UI/ADB journey with persisted events independent of visible status."""
    completed_id = "11111111-1111-1111-1111-111111111111"
    paused_id = "22222222-2222-2222-2222-222222222222"
    other_id = "33333333-3333-3333-3333-333333333333"

    def __init__(self, fault=None, polished=False, library=False):
        self.polished = polished
        self.library = library
        self.disclosed = set()
        self.confirmations = 0
        self.fault = fault
        self.screen = "list"
        self.installed = []
        self.runs = {"old-preserved-run"}
        self.transfers = {}
        self.targets = {}
        self.calls = []
        self.types = ["model_call", "paused"]
        self.selected = None
        self.resume_taps = 0
        self.start_taps = 0
        self.action_log = []
        self.ui_generation = 0

    def digest(self, name):
        paused = name == flow_smoke.fixture.PAUSED_FLOW_ID
        return hashlib.sha256(flow_smoke.fixture.fixture_files(paused=paused)[f"{name}.flow.yaml"]).hexdigest()

    def ui(self):
        root = ET.Element("hierarchy")
        self.targets = {}
        self.ui_generation += 1

        def node(text, action=None, package=flow_smoke.PACKAGE, enabled=True, parent=None, description=None):
            # Menus/dialogs get fresh coordinates after every capture.
            y = 20 + len(self.targets) * 40 + (self.ui_generation % 17 if self.library else 0)
            bounds = f"[0,{y}][200,{y + 30}]"
            result = ET.SubElement(root if parent is None else parent, "node", package=package, text=text,
                                   enabled=str(enabled).lower(), bounds=bounds)
            if action is not None:
                result.set("clickable", "true")
            if description is not None:
                result.set("content-desc", description)
            self.targets[("100", str(y + 15))] = action
            return result

        def up():
            if self.library:
                node("", "back", description="Navigate up")
            else:
                node("‹ Back" if self.polished else "Back", "back")

        if self.screen == "list" and self.library:
            node("", "manage", description="Manage flows")
            for name in self.installed:
                for candidate in ("wrong-flow", "flow"):
                    node(name, "ambiguous-primary")
                    node("", ("library-details", candidate, name), description=f"Details for {name}")
            if self.completed_id in self.runs:
                for run_id in (self.other_id, self.completed_id):
                    node(f"{flow_smoke.fixture.FLOW_ID}\nSame date", "ambiguous-primary")
                    node("", ("library-details", "run", run_id),
                         description=f"Details for {flow_smoke.fixture.FLOW_ID}")
        elif self.screen == "list":
            node("Install flow" if self.polished else "Install bundle", "install")
            for name in self.installed:
                card = node("", ("flow", name))
                ET.SubElement(card, "node", text=name)
                digest = self.digest(name)
                ET.SubElement(card, "node", text="Version 1" if self.polished else f"v1 · sha256 {digest[:12]}…")
            if self.completed_id in self.runs:
                if self.polished:
                    for run_id in (self.other_id, self.completed_id):
                        card = node("", "history" if run_id == self.completed_id else "wrong-history")
                        ET.SubElement(card, "node", text="Same title and date")
                        if run_id in self.disclosed:
                            node(f"Run ID: {run_id}\nFlow ID: android-smoke", parent=card)
                        else:
                            node("Technical details", ("disclose", run_id), parent=card)
                else:
                    card = node("", "history")
                    ET.SubElement(card, "node", text=self.completed_id)
        elif self.screen == "manage":
            node("Import flow file", "install")
        elif self.screen == "library-details":
            kind, value = self.selection
            # A dump can contain background controls on another window.
            button = "View run" if kind == "run" else "Open flow"
            node(button, "ambiguous-primary")
            dialog = ET.SubElement(root, "node", package=flow_smoke.PACKAGE)
            if kind == "run":
                run_id = self.other_id if self.fault == "wrong-history-uuid" else value
                node(f"Run ID: {run_id}\nFlow ID: {flow_smoke.fixture.FLOW_ID}", parent=dialog)
                node(button, ("open-run", value), parent=dialog, enabled=self.fault != "disabled-view-run")
            else:
                digest = "0" * 64 if kind == "wrong-flow" or self.fault == "wrong-flow-sha" else self.digest(value)
                node(f"Version 1\nFlow ID: {value}\nFlow SHA-256: {digest}", parent=dialog)
                node(button, ("flow", value), parent=dialog, enabled=self.fault != "disabled-open-flow")
            node("Done", "dismiss", parent=dialog)
        elif self.screen == "install":
            if self.library:
                node("Import flow file")
            node("Choose file", "choose")
        elif self.screen == "picker":
            for remote in self.transfers:
                node(Path(remote).name, ("file", remote), flow_smoke.DOCUMENTS[0])
        elif self.screen == "installed":
            if self.library and "Version" in self.install_result:
                node("Flow installed")
            node(self.install_result)
            up()
        elif self.screen == "setup":
            node("Start run", "start", enabled=self.fault != "disabled-start")
            if self.polished:
                if "setup" not in self.disclosed:
                    node("Technical details", ("disclose", "setup"))
                else:
                    node("Hide technical details", ("hide", "setup"))
                    digest = self.digest(self.selected)
                    if self.fault == "wrong-flow-sha":
                        digest = "0" * 64
                    node(f"Flow ID: {self.selected}\nBundle SHA-256: {digest}")
            elif not self.library:
                digest = self.digest(self.selected)
                if self.fault == "wrong-flow-sha":
                    digest = digest[:12] + "0" * 52
                node(f"Bundle SHA-256: {digest}")
        else:
            paused = self.selected == flow_smoke.fixture.PAUSED_FLOW_ID
            if self.library:
                node("Technical details", ("toggle", "run"))
                if "run" in self.disclosed:
                    run_id = self.other_id if self.fault == "wrong-cold-uuid" and self.screen == "interrupted" else (
                        self.paused_id if paused else self.completed_id)
                    node(f"Run ID · {run_id}\nFlow ID · {self.selected}")
            elif self.polished:
                if "run" in self.disclosed:
                    node("Hide technical details", ("hide", "run"))
                    identity = self.paused_id if paused else self.completed_id
                    node(f"Run ID: {identity}")
                else:
                    node("Technical details", ("disclose", "run"))
            else:
                node(self.paused_id if paused else self.completed_id)
            if self.screen == "complete":
                node("succeeded" if self.fault == "wrong-success-status" else "Completed" if self.polished or self.library else "Succeeded")
                node(flow_smoke.fixture.EXPECTED_REPORT)
            elif self.screen == "paused":
                if self.library:
                    node("Paused")
                    node(flow_smoke.fixture.PAUSE_REASON)
                else:
                    node("Paused: needs-attention")
            elif self.screen == "interrupted":
                if self.library:
                    node("Interrupted")
                    node("Restore progress using the saved run.")
                else:
                    node("Paused: needs-attention" if self.fault == "paused-after-cold-open" else
                         "Interrupted — tap Resume to restore saved progress. Models load when needed.")
            elif self.screen == "cancelled":
                node("failed: cancelled" if self.fault == "wrong-cancel-status" else "Cancelled")
            if self.screen in ("paused", "interrupted"):
                if self.fault != "hidden-resume":
                    node("Resume saved run" if self.library else "Resume", "resume",
                         enabled=self.fault != "disabled-resume")
                if self.library:
                    if self.screen == "paused":
                        node("", "run-actions", description="Run actions")
                else:
                    node("Cancel run" if self.polished else "Cancel", "cancel")
            elif self.screen == "run-actions":
                node("Cancel run", "cancel", enabled=self.fault != "disabled-menu-cancel")
            elif self.screen == "confirm-cancel":
                node("Cancel this run?")
                node("Cancel run", "confirm-cancel", enabled=self.fault != "disabled-confirm-cancel")
                node("Keep run", "keep-run")
            up()
        return '<?xml version="1.0"?>' + ET.tostring(root, encoding="unicode")

    def adb(self, *args, **kwargs):
        self.calls.append(args)
        if args[:2] == ("exec-out", "uiautomator"):
            return self.ui()
        if args[0] == "push":
            self.transfers[args[2]] = Path(args[1]).name
        elif args[:3] == ("shell", "input", "tap"):
            action = self.targets[args[3:5]]
            self.action_log.append(action)
            if action == "manage":
                self.screen = "manage"
            elif action == "install":
                self.screen = "install"
            elif action == "choose":
                self.screen = "picker"
            elif isinstance(action, tuple) and action[0] == "file":
                name = self.transfers[action[1]]
                if "bad-hash" in name:
                    self.install_result = "Install rejected: sha256 mismatch"
                else:
                    name = name.removesuffix(".tar")
                    self.installed.append(name)
                    self.install_result = f"{name} · Version 1\n2 files verified against the bundle manifest." if self.polished or self.library else f"Installed: {name} v1\nsha256: test\nfiles verified: 2\nsignatures: unsigned"
                self.screen = "installed"
            elif isinstance(action, tuple) and action[0] == "library-details":
                self.selection = action[1:]
                self.screen = "library-details"
            elif action == "dismiss":
                self.screen = "list"
            elif isinstance(action, tuple) and action[0] == "flow":
                self.selected = action[1]
                self.disclosed.discard("setup")
                self.screen = "setup"
            elif isinstance(action, tuple) and action[0] == "disclose":
                self.disclosed.add(action[1])
            elif isinstance(action, tuple) and action[0] == "hide":
                self.disclosed.discard(action[1])
            elif isinstance(action, tuple) and action[0] == "toggle":
                self.disclosed ^= {action[1]}
            elif action == "start":
                self.start_taps += 1
                self.disclosed.discard("run")
                paused = self.selected == flow_smoke.fixture.PAUSED_FLOW_ID
                self.runs.add(self.paused_id if paused else self.completed_id)
                self.screen = "paused" if paused else "complete"
                if paused and self.fault == "missing-initial-events":
                    self.types = []
            elif action in ("history", ("open-run", self.completed_id)):
                self.selected = flow_smoke.fixture.FLOW_ID
                self.disclosed.discard("run")
                self.screen = "complete"
            elif action == "back":
                self.screen = "list"
            elif action == "resume":
                self.resume_taps += 1
                if self.fault != "stale-resume-events":
                    self.types += ["resumed"]
                    if self.fault != "missing-resumed-model":
                        self.types += ["model_call"]
                    self.types += ["paused"]
                self.screen = "paused"
            elif action == "run-actions":
                self.screen = "run-actions"
            elif action == "cancel" and (self.polished or self.library):
                self.screen = "confirm-cancel"
            elif action in ("cancel", "confirm-cancel"):
                if action == "confirm-cancel":
                    self.confirmations += 1
                if self.fault != "missing-terminal-event":
                    self.types += ["run_ended"]
                self.screen = "cancelled"
            else:
                raise AssertionError(f"Unexpected tap: {action}")
        elif args[:3] == ("shell", "am", "force-stop"):
            self.disclosed.discard("run")
            if self.screen == "paused":
                self.screen = "interrupted"
                if self.fault == "auto-resumed-on-cold-open":
                    self.types += ["resumed", "model_call", "paused"]
        elif args[:4] == ("shell", "run-as", flow_smoke.PACKAGE, "ls"):
            return "\n".join(self.runs if args[4].endswith("runs") else self.installed)
        elif args[:4] == ("shell", "run-as", flow_smoke.PACKAGE, "head"):
            if self.fault == "invalid-cold-journal" and self.screen == "interrupted":
                return "not JSON"
            return "\n".join(json.dumps({"type": kind, **({"outcome": "cancelled"} if kind == "run_ended" else {})})
                             for kind in self.types)
        elif args[:3] not in (("shell", "am", "start"), ("shell", "input", "keyevent"), ("shell", "rm", "-f")):
            raise AssertionError(f"Unexpected ADB call: {args}")
        return ""


class ScrollingFlowDevice(FakeFlowDevice):
    """Two simulated viewports: controls above disclosed identities/reports."""
    def __init__(self):
        super().__init__(polished=True)
        self.screen = "install"  # Restored Install title must not count as home.
        self.page = 1
        self.page_screen = self.screen
        self.dialog_delay = 0
        self.scroll_directions = set()

    def ui(self):
        if self.dialog_delay:
            self.dialog_delay -= 1
            self.screen = "paused"
            output = super().ui()
            self.screen = "confirm-cancel"
            return output
        if self.page_screen != self.screen:
            self.page = 0
            self.page_screen = self.screen
        root = flow_smoke.parse_ui(super().ui())
        if self.screen not in ("install", "installed", "setup", "complete", "paused", "interrupted", "cancelled"):
            return '<?xml version="1.0"?>' + ET.tostring(root, encoding="unicode")
        if self.screen == "install":
            ET.SubElement(root, "node", package=flow_smoke.PACKAGE, text="Install flow", enabled="true")
            back = ET.SubElement(root, "node", package=flow_smoke.PACKAGE, text="‹ Back",
                                 clickable="true", enabled="true", bounds="[220,20][420,50]")
            self.targets[flow_smoke.center(back)] = "back"
        for node in list(root):
            text = node.get("text", "")
            is_top = text in ("Install flow", "‹ Back", "Start run", "Resume", "Cancel run", "Completed", "Cancelled") or text.startswith(("Paused:", "Interrupted"))
            if is_top != (self.page == 0):
                root.remove(node)
        ET.SubElement(root, "node", package=flow_smoke.PACKAGE, enabled="true",
                      scrollable="true", bounds="[20,100][420,700]")
        if self.page == 1 and "run" in self.disclosed:
            ET.SubElement(root, "node", package=flow_smoke.PACKAGE, enabled="true",
                          scrollable="true", bounds="[20,400][420,650]")
        return '<?xml version="1.0"?>' + ET.tostring(root, encoding="unicode")

    def adb(self, *args, **kwargs):
        if args[:3] == ("shell", "input", "swipe"):
            self.calls.append(args)
            self.page = 1 if int(args[4]) > int(args[6]) else 0
            self.scroll_directions.add("up" if self.page else "down")
            return ""
        action = self.targets.get(args[3:5]) if args[:3] == ("shell", "input", "tap") else None
        result = super().adb(*args, **kwargs)
        if action == "cancel":
            self.dialog_delay = 1
        return result


class FlowSmokeJourney(unittest.TestCase):
    def exercise(self, device):
        with patch.object(flow_smoke.fixture, "main"), patch.object(flow_smoke.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            return flow_smoke.exercise_flow(device.adb)

    def test_current_statuses_cold_interruption_and_explicit_resume_complete_journey(self):
        device = FakeFlowDevice()
        checks = self.exercise(device)
        self.assertIn("completed-run-cold-recovery", checks)
        self.assertIn("paused-run-cold-recovery", checks)
        self.assertIn("resume-reruns-task", checks)
        self.assertIn("cancel-paused-run", checks)
        self.assertEqual(device.resume_taps, 1)
        self.assertEqual(device.runs, {"old-preserved-run", device.completed_id, device.paused_id})
        pushed = [call[2] for call in device.calls if call[0] == "push"]
        removed = [call[-1] for call in device.calls if call[:3] == ("shell", "rm", "-f")]
        self.assertEqual(removed, pushed)

    def test_polished_ui_verifies_hidden_identities_and_confirms_cancel(self):
        device = FakeFlowDevice(polished=True)
        checks = self.exercise(device)
        self.assertIn("history-reopen", checks)
        self.assertIn("completed-run-cold-recovery", checks)
        self.assertIn("cancel-paused-run", checks)
        self.assertEqual(device.confirmations, 1)
        self.assertIn("33333333-3333-3333-3333-333333333333", device.disclosed)
        self.assertIn(device.completed_id, device.disclosed)
        self.assertEqual(device.runs, {"old-preserved-run", device.completed_id, device.paused_id})

    def test_scrolled_polished_journey_restores_install_and_reaches_top_controls(self):
        device = ScrollingFlowDevice()
        checks = self.exercise(device)
        self.assertIn("completed-run-cold-recovery", checks)
        self.assertIn("cancel-paused-run", checks)
        self.assertEqual(device.confirmations, 1)
        self.assertEqual(device.scroll_directions, {"up", "down"})
        self.assertEqual(device.runs, {"old-preserved-run", device.completed_id, device.paused_id})

    def test_library_journey_opens_verified_dialog_actions_and_reviews_duplicate_rows(self):
        device = FakeFlowDevice(library=True)
        checks = self.exercise(device)
        self.assertIn("history-reopen", checks)
        self.assertIn("completed-run-cold-recovery", checks)
        self.assertIn("paused-run-cold-recovery", checks)
        self.assertIn("resume-reruns-task", checks)
        self.assertIn("cancel-paused-run", checks)
        self.assertEqual(device.start_taps, 2)
        self.assertEqual(device.resume_taps, 1)
        self.assertEqual(device.confirmations, 1)
        self.assertEqual(device.action_log[:3], ["manage", "install", "choose"])
        self.assertEqual(device.action_log[-3:], ["run-actions", "cancel", "confirm-cancel"])
        self.assertIn(("library-details", "wrong-flow", flow_smoke.fixture.FLOW_ID), device.action_log)
        self.assertIn(("library-details", "run", device.other_id), device.action_log)
        self.assertIn(("open-run", device.completed_id), device.action_log)
        self.assertNotIn(("open-run", device.other_id), device.action_log)
        self.assertNotIn("ambiguous-primary", device.action_log)
        self.assertEqual(device.runs, {"old-preserved-run", device.completed_id, device.paused_id})

    def test_library_identity_and_disabled_controls_fail_before_side_effects(self):
        for fault, start_taps, resume_taps in (
            ("wrong-flow-sha", 0, 0),
            ("disabled-open-flow", 0, 0),
            ("disabled-start", 0, 0),
            ("wrong-history-uuid", 1, 0),
            ("disabled-view-run", 1, 0),
            ("wrong-cold-uuid", 2, 0),
            ("disabled-resume", 2, 0),
            ("disabled-menu-cancel", 2, 1),
            ("disabled-confirm-cancel", 2, 1),
        ):
            with self.subTest(fault=fault):
                device = FakeFlowDevice(fault, library=True)
                with self.assertRaises(FlowSmokeError):
                    self.exercise(device)
                self.assertEqual(device.start_taps, start_taps)
                self.assertEqual(device.resume_taps, resume_taps)
                self.assertEqual(device.confirmations, 0)
                self.assertNotIn("ambiguous-primary", device.action_log)
                if fault in ("wrong-history-uuid", "disabled-view-run"):
                    self.assertNotIn(("open-run", device.completed_id), device.action_log)
                pushed = [call[2] for call in device.calls if call[0] == "push"]
                removed = [call[-1] for call in device.calls if call[:3] == ("shell", "rm", "-f")]
                self.assertEqual(removed, pushed)

    def test_polished_wrong_full_flow_sha_stops_before_start_and_preserves_runs(self):
        for polished in (False, True):
            with self.subTest(polished=polished):
                device = FakeFlowDevice("wrong-flow-sha", polished=polished)
                with self.assertRaisesRegex(FlowSmokeError, "identity"):
                    self.exercise(device)
                self.assertEqual(device.runs, {"old-preserved-run"})
                self.assertEqual(device.start_taps, 0)
                self.assertEqual(device.resume_taps, 0)
                pushed = [call[2] for call in device.calls if call[0] == "push"]
                removed = [call[-1] for call in device.calls if call[:3] == ("shell", "rm", "-f")]
                self.assertEqual(removed, pushed)

    def test_wrong_status_missing_stale_and_unrequested_execution_fail_with_cleanup(self):
        for fault in ("wrong-success-status", "wrong-cancel-status", "missing-initial-events",
                      "disabled-resume", "hidden-resume", "paused-after-cold-open", "invalid-cold-journal",
                      "auto-resumed-on-cold-open", "stale-resume-events", "missing-resumed-model", "missing-terminal-event"):
            with self.subTest(fault=fault):
                device = FakeFlowDevice(fault)
                with self.assertRaises(FlowSmokeError):
                    self.exercise(device)
                if fault in ("disabled-resume", "hidden-resume", "paused-after-cold-open", "invalid-cold-journal",
                             "auto-resumed-on-cold-open", "missing-initial-events"):
                    self.assertEqual(device.resume_taps, 0)
                pushed = [call[2] for call in device.calls if call[0] == "push"]
                removed = [call[-1] for call in device.calls if call[:3] == ("shell", "rm", "-f")]
                self.assertEqual(removed, pushed)


if __name__ == "__main__":
    unittest.main()
