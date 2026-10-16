"""UI control selection guards; no Android device or SDK required."""

import unittest
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


if __name__ == "__main__":
    unittest.main()
