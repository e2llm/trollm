"""Offline guard tests; no SDK, native build, device, or network required."""

import argparse
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import smoke


class SmokeGuards(unittest.TestCase):
    def setUp(self):
        cache = Path(__file__).resolve().parents[2] / ".cache"
        cache.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=cache)
        self.addCleanup(self.temp.cleanup)
        self.apk = Path(self.temp.name) / "fixture.apk"
        with zipfile.ZipFile(self.apk, "w") as archive:
            for lib in ("libtrollm_uniffi.so", "libjnidispatch.so"):
                archive.writestr(f"lib/x86_64/{lib}", b"fixture")
        self.args = argparse.Namespace(apk=self.apk, aapt="aapt", adb="adb",
                                      serial="emulator-5554", socket="tcp:127.0.0.1:5038",
                                      boot_timeout=1, reset_app_data=False)

    def fake(self, device_abi="x86_64", sdk="36", package=smoke.PACKAGE):
        calls = []

        def command(argv, timeout=20, env=None):
            calls.append(argv)
            if argv[0] == "aapt":
                return f"package: name='{package}'\napplication-debuggable\n"
            self.assertEqual(argv[:3], ["adb", "-s", self.args.serial])
            self.assertEqual(env["ADB_SERVER_SOCKET"], self.args.socket)
            tail = argv[3:]
            if tail == ["get-state"]:
                return "device"
            if tail[:2] == ["shell", "getprop"]:
                return {"sys.boot_completed": "1", "ro.build.version.sdk": sdk,
                        "ro.product.cpu.abilist": device_abi}[tail[2]]
            if tail[0] == "install":
                return "Success"
            if tail[:3] == ["shell", "am", "start"]:
                return "Status: ok"
            if tail[:2] == ["shell", "pidof"]:
                return "1234"
            if tail[0] == "exec-out":
                return '<?xml version="1.0"?><hierarchy><node package="com.trollm.app" text="Trollm" /></hierarchy>'
            return ""
        return command, calls

    def test_wrong_apk_refused_before_adb(self):
        fake, calls = self.fake(package="com.example.other")
        with patch.object(smoke, "command", fake), self.assertRaises(smoke.SmokeError):
            smoke.smoke(self.args)
        self.assertEqual(len(calls), 1)

    def test_unsupported_api_and_abi_refused_before_install(self):
        for abi, sdk in [("arm64-v8a", "36"), ("x86_64", "32")]:
            fake, calls = self.fake(device_abi=abi, sdk=sdk)
            with patch.object(smoke, "command", fake), self.assertRaises(smoke.SmokeError):
                smoke.smoke(self.args)
            self.assertFalse(any("install" in argv for argv in calls))

    def test_missing_native_pair_refused_before_adb(self):
        with zipfile.ZipFile(self.apk, "w") as archive:
            archive.writestr("lib/x86_64/libtrollm_uniffi.so", b"fixture")
        fake, calls = self.fake()
        with patch.object(smoke, "command", fake), self.assertRaises(smoke.SmokeError):
            smoke.smoke(self.args)
        self.assertEqual(len(calls), 1)

    def test_boot_deadline_refuses_install(self):
        fake, calls = self.fake()

        def offline(argv, timeout=20, env=None):
            if argv[0] == "adb":
                raise smoke.SmokeError("offline")
            return fake(argv, timeout, env)

        with patch.object(smoke, "command", offline), patch.object(smoke.time, "monotonic", side_effect=[0, 2]):
            with self.assertRaises(smoke.SmokeError):
                smoke.smoke(self.args)
        self.assertFalse(any("install" in argv for argv in calls))

    def test_full_smoke_targets_only_trollm(self):
        fake, calls = self.fake()
        with patch.object(smoke, "command", fake), patch.object(smoke.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            result = smoke.smoke(self.args)
        self.assertEqual(result["status"], "passed")
        self.assertIsNone(result["sourceCommit"])
        self.assertEqual(result["backend"], "unknown")
        self.assertEqual(result["deviceApi"], 36)
        self.assertEqual(sum("install" in argv for argv in calls), 1)
        self.assertFalse(any("clear" in argv or "reboot" in argv or "uninstall" in argv for argv in calls))
        stops = [argv for argv in calls if "force-stop" in argv]
        self.assertEqual(len(stops), 2)
        self.assertTrue(all(argv[-1] == smoke.PACKAGE for argv in stops))
        launches = [argv for argv in calls if argv[3:6] == ["shell", "am", "start"]]
        self.assertTrue(all("--activity-clear-top" in argv for argv in launches))

    def test_ui_error_does_not_disclose_text(self):
        with self.assertRaises(smoke.SmokeError) as raised:
            smoke.check_ui('<?xml version="1.0"?><hierarchy><node package="com.trollm.app" text="Error: SECRET" /></hierarchy>')
        self.assertNotIn("SECRET", str(raised.exception))

    def test_hung_adb_is_bounded_and_output_hidden(self):
        with patch.object(smoke.subprocess, "run", side_effect=subprocess.TimeoutExpired("adb", 5, output="SECRET")):
            with self.assertRaises(smoke.SmokeError) as raised:
                smoke.command(["adb", "get-state"], timeout=5)
        self.assertNotIn("SECRET", str(raised.exception))

    def test_build_provenance_binds_source_to_exact_apk(self):
        build_info = self.apk.with_suffix(".json")
        provenance = {"schemaVersion": 1, **smoke.apk_provenance(self.apk),
                      "sourceCommit": "a" * 40, "backend": "stub", "unexpected": "SECRET"}
        build_info.write_text(json.dumps(provenance))
        result = smoke.apk_provenance(self.apk, build_info)
        self.assertEqual(result["sourceCommit"], "a" * 40)
        self.assertEqual(result["backend"], "stub")
        self.assertNotIn("SECRET", json.dumps(result))
        self.apk.write_bytes(b"changed APK")
        with self.assertRaises(smoke.SmokeError):
            smoke.apk_provenance(self.apk, build_info)

    def test_invalid_provenance_refuses_device_actions(self):
        self.args.build_info = self.apk.with_suffix(".json")
        for content in ("{}", "null", "bad JSON"):
            self.args.build_info.write_text(content)
            fake, calls = self.fake()
            with patch.object(smoke, "command", fake), self.assertRaises(smoke.SmokeError):
                smoke.smoke(self.args)
            self.assertEqual(len(calls), 1)

    def test_deterministic_fixture_refuses_known_llama_backend(self):
        self.args.exercise_flow = True
        fake, calls = self.fake()
        with patch.object(smoke, "command", fake), patch.object(smoke, "apk_provenance", return_value={"backend": "llama"}):
            with self.assertRaises(smoke.SmokeError):
                smoke.smoke(self.args)
        self.assertEqual(len(calls), 1)

    def test_deterministic_fixture_requires_explicit_historical_override(self):
        self.args.exercise_flow = True
        fake, calls = self.fake()
        with patch.object(smoke, "command", fake), self.assertRaises(smoke.SmokeError):
            smoke.smoke(self.args)
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
