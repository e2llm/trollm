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

    def open_fixture(self, roots):
        events = []
        frames = iter(roots)

        def capture():
            events.append("capture")
            return next(frames)

        def adb(*parts):
            events.append(parts)

        return events, lambda: open_fixture_card(adb, capture, "android-smoke", "a" * 64)

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


class FakeFlowDevice:
    """Small UI/ADB journey with persisted events independent of visible status."""
    completed_id = "11111111-1111-1111-1111-111111111111"
    paused_id = "22222222-2222-2222-2222-222222222222"

    def __init__(self, fault=None):
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

    def ui(self):
        root = ET.Element("hierarchy")
        self.targets = {}

        def node(text, action=None, package=flow_smoke.PACKAGE, enabled=True):
            y = 20 + len(self.targets) * 40
            bounds = f"[0,{y}][200,{y + 30}]"
            result = ET.SubElement(root, "node", package=package, text=text,
                                   enabled=str(enabled).lower(), bounds=bounds)
            if action is not None:
                result.set("clickable", "true")
            self.targets[("100", str(y + 15))] = action
            return result

        if self.screen == "list":
            node("Install bundle", "install")
            for name in self.installed:
                card = node("", ("flow", name))
                ET.SubElement(card, "node", text=name)
                paused = name == flow_smoke.fixture.PAUSED_FLOW_ID
                digest = hashlib.sha256(flow_smoke.fixture.fixture_files(paused=paused)[f"{name}.flow.yaml"]).hexdigest()
                ET.SubElement(card, "node", text=f"v1 · sha256 {digest[:12]}…")
            if self.completed_id in self.runs:
                card = node("", "history")
                ET.SubElement(card, "node", text=self.completed_id)
        elif self.screen == "install":
            node("Choose file", "choose")
        elif self.screen == "picker":
            for remote in self.transfers:
                node(Path(remote).name, ("file", remote), flow_smoke.DOCUMENTS[0])
        elif self.screen == "installed":
            node(self.install_result)
            node("Back", "back")
        elif self.screen == "setup":
            node("Start run", "start")
        else:
            paused = self.selected == flow_smoke.fixture.PAUSED_FLOW_ID
            node(self.paused_id if paused else self.completed_id)
            if self.screen == "complete":
                node("succeeded" if self.fault == "wrong-success-status" else "Succeeded")
                node(flow_smoke.fixture.EXPECTED_REPORT)
            elif self.screen == "paused":
                node("Paused: needs-attention")
            elif self.screen == "interrupted":
                node("Paused: needs-attention" if self.fault == "paused-after-cold-open" else
                     "Interrupted — tap Resume to restore saved progress. Models load when needed.")
            elif self.screen == "cancelled":
                node("failed: cancelled" if self.fault == "wrong-cancel-status" else "Cancelled")
            if self.screen in ("paused", "interrupted"):
                if self.fault != "hidden-resume":
                    node("Resume", "resume", enabled=self.fault != "disabled-resume")
                node("Cancel", "cancel")
            node("Back", "back")
        return '<?xml version="1.0"?>' + ET.tostring(root, encoding="unicode")

    def adb(self, *args, **kwargs):
        self.calls.append(args)
        if args[:2] == ("exec-out", "uiautomator"):
            return self.ui()
        if args[0] == "push":
            self.transfers[args[2]] = Path(args[1]).name
        elif args[:3] == ("shell", "input", "tap"):
            action = self.targets[args[3:5]]
            if action == "install":
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
                    self.install_result = f"Installed: {name}"
                self.screen = "installed"
            elif isinstance(action, tuple) and action[0] == "flow":
                self.selected = action[1]
                self.screen = "setup"
            elif action == "start":
                paused = self.selected == flow_smoke.fixture.PAUSED_FLOW_ID
                self.runs.add(self.paused_id if paused else self.completed_id)
                self.screen = "paused" if paused else "complete"
                if paused and self.fault == "missing-initial-events":
                    self.types = []
            elif action == "history":
                self.selected = flow_smoke.fixture.FLOW_ID
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
            elif action == "cancel":
                if self.fault != "missing-terminal-event":
                    self.types += ["run_ended"]
                self.screen = "cancelled"
            else:
                raise AssertionError(f"Unexpected tap: {action}")
        elif args[:3] == ("shell", "am", "force-stop"):
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
