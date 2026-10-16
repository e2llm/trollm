#!/usr/bin/env python3
"""Bounded install/JNI/UI smoke on an explicitly selected Android target."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
import zipfile

PACKAGE = "com.trollm.app"
ACTIVITY = f"{PACKAGE}/.MainActivity"


class SmokeError(Exception):
    pass


def command(argv, timeout=20, env=None):
    label = Path(argv[0]).name
    if label == "adb" and len(argv) > 3 and argv[1] == "-s":
        label += " " + argv[3]
        if argv[3] in ("shell", "exec-out") and len(argv) > 4:
            label += "/" + argv[4]
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=timeout, env=env)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise SmokeError(f"{label} failed or exceeded {timeout}s") from exc
    if result.returncode:
        # Never print tool output: ADB diagnostics and UI may contain credentials.
        raise SmokeError(f"{label} command failed (exit {result.returncode})")
    return result.stdout.replace("\r", "")


def apk_info(apk, aapt):
    badging = command([aapt, "dump", "badging", str(apk)])
    match = re.search(r"^package: name='([^']+)'", badging, re.M)
    if not match or match[1] != PACKAGE:
        raise SmokeError(f"APK must declare package {PACKAGE}; installation refused")
    if not re.search(r"^application-debuggable(?:\s|$)", badging, re.M):
        raise SmokeError("APK must be debuggable for the private JNI storage check")
    with zipfile.ZipFile(apk) as archive:
        names = set(archive.namelist())
    abis = {
        abi for abi in ("arm64-v8a", "armeabi-v7a", "x86_64")
        if all(f"lib/{abi}/{lib}" in names for lib in
               ("libtrollm_uniffi.so", "libjnidispatch.so"))
    }
    if not abis:
        raise SmokeError("APK lacks a complete Trollm/JNA native library pair")
    return abis


def apk_provenance(apk, build_info=None):
    hasher = hashlib.sha256()
    with apk.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    result = {"apkSha256": digest, "sourceCommit": None, "backend": "unknown"}
    if build_info is not None:
        try:
            supplied = json.loads(build_info.read_text())
            valid = (isinstance(supplied, dict) and supplied.get("schemaVersion") == 1
                     and supplied.get("apkSha256") == digest
                     and isinstance(supplied.get("sourceCommit"), str)
                     and re.fullmatch(r"[0-9a-f]{40}", supplied["sourceCommit"])
                     and supplied.get("backend") in ("stub", "llama"))
        except (OSError, ValueError):
            valid = False
        if not valid:
            raise SmokeError("Build provenance is invalid or does not match the APK")
        result.update({key: supplied[key] for key in ("sourceCommit", "backend")})
    return result


def check_ui(xml):
    start, end = xml.find("<?xml"), xml.rfind("</hierarchy>")
    if start < 0 or end < 0:
        raise SmokeError("Could not capture Android UI hierarchy")
    root = ET.fromstring(xml[start:end + len("</hierarchy>")])
    nodes = [node for node in root.iter("node") if node.get("package") == PACKAGE]
    if not nodes:
        raise SmokeError("Trollm UI is not visible; unlock target and retry")
    if any(node.get("text", "").startswith("Error:") for node in nodes):
        raise SmokeError("Trollm displays a runtime error (details omitted)")


def smoke(args):
    abis = apk_info(args.apk, args.aapt)  # Validate before any ADB action.
    report = {"schemaVersion": 1, **apk_provenance(args.apk, getattr(args, "build_info", None))}
    if getattr(args, "exercise_flow", False) and report["backend"] != "stub":
        if report["backend"] == "unknown" and getattr(args, "allow_unverified_stub", False):
            report["backendOverride"] = "operator-declared-historical-stub"
        else:
            raise SmokeError("Flow smoke requires matching stub build-info; only a known historical stub may use --allow-unverified-stub")
    env = os.environ.copy()
    env["ADB_SERVER_SOCKET"] = args.socket

    def adb(*argv, timeout=20):
        return command([args.adb, "-s", args.serial, *argv], timeout, env)

    deadline = time.monotonic() + args.boot_timeout
    while True:
        try:
            online = adb("get-state", timeout=5).strip() == "device"
            booted = online and adb("shell", "getprop", "sys.boot_completed", timeout=5).strip() == "1"
            if booted:
                break
        except SmokeError:
            pass
        if time.monotonic() >= deadline:
            raise SmokeError("Selected device did not finish boot before deadline; start simulator/tunnel")
        time.sleep(min(1, max(0, deadline - time.monotonic())))
    sdk = adb("shell", "getprop", "ro.build.version.sdk").strip()
    if not sdk.isdigit() or int(sdk) < 33:
        raise SmokeError("Selected device requires API 33 or newer")
    device_abis = set(adb("shell", "getprop", "ro.product.cpu.abilist").strip().split(","))
    if not abis.intersection(device_abis):
        raise SmokeError("APK native libraries do not match selected device ABI")
    report.update({"deviceApi": int(sdk), "deviceAbis": sorted(device_abis),
                   "apkAbis": sorted(abis), "checks": []})
    print(f"PASS: target API {sdk}; compatible Trollm/JNA native libraries")
    if "Success" not in adb("install", "-r", "-t", str(args.apk), timeout=120):
        raise SmokeError("ADB did not report successful APK installation")
    print("PASS: Trollm APK installed")
    if args.reset_app_data:
        if adb("shell", "pm", "clear", PACKAGE).strip() != "Success":
            raise SmokeError("Trollm data reset failed")
        print("PASS: only Trollm app data reset")

    def launch():
        # A pending DocumentsUI activity can survive above the stopped app's
        # activity record. Clear that task's upper activities, not other apps.
        output = adb("shell", "am", "start", "-W", "--activity-clear-top", "-n", ACTIVITY, timeout=30)
        if "Status: ok" not in output:
            raise SmokeError("Trollm launch failed")
        time.sleep(2)
        if not adb("shell", "pidof", PACKAGE).strip():
            raise SmokeError("Trollm process exited after launch")
        check_ui(adb("exec-out", "uiautomator", "dump", "/dev/tty", timeout=30))
        # MainActivity constructs RuntimeHolder before setting Compose content.
        # Presence of its private storage tree confirms the native runtime opened.
        adb("shell", "run-as", PACKAGE, "test", "-d", "files/trollm")

    adb("shell", "am", "force-stop", PACKAGE)
    launch()
    print("PASS: cold launch, visible UI, UniFFI runtime storage")
    adb("shell", "input", "keyevent", "KEYCODE_HOME")
    launch()
    print("PASS: background/foreground UI")
    adb("shell", "am", "force-stop", PACKAGE)
    launch()
    print("PASS: process restart and native runtime reopen")
    report["checks"].extend(["install", "native-startup", "visible-ui", "background-foreground", "process-restart"])
    if getattr(args, "exercise_flow", False):
        from flow_smoke import FlowSmokeError, exercise_flow
        try:
            report["checks"].extend(exercise_flow(adb))
        except FlowSmokeError as exc:
            raise SmokeError(str(exc)) from exc
    print("Smoke complete; real inference and active-run lifecycle require separate checks.")
    report["status"] = "passed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", type=Path, required=True, help="Existing debug APK; no build/download")
    parser.add_argument("--serial", default=os.getenv("TROLLM_DEVICE_SERIAL"))
    parser.add_argument("--socket", default=os.getenv("TROLLM_ADB_SERVER_SOCKET"))
    parser.add_argument("--adb", default=os.getenv("TROLLM_ADB", shutil.which("adb") or "adb"))
    parser.add_argument("--aapt", default=os.getenv("TROLLM_AAPT", shutil.which("aapt") or "aapt"))
    parser.add_argument("--boot-timeout", type=int, default=60)
    parser.add_argument("--reset-app-data", action="store_true", help="Delete ONLY Trollm data")
    parser.add_argument("--build-info", type=Path, help="CI build-info.json matching this APK")
    parser.add_argument("--report", type=Path, help="Write a sanitized local JSON result; never a CI gate")
    parser.add_argument("--exercise-flow", action="store_true", help="Install and run the tiny built-in stub fixture through the UI")
    parser.add_argument("--allow-unverified-stub", action="store_true", help="Allow a known historical stub APK without CI provenance; source/backend remain unverified in the report")
    args = parser.parse_args()
    if not args.serial or not re.fullmatch(r"[A-Za-z0-9_.:\[\]-]+", args.serial):
        parser.error("provide an explicit --serial or TROLLM_DEVICE_SERIAL")
    if not args.socket or not args.socket.startswith(("tcp:", "localfilesystem:", "localabstract:")):
        parser.error("provide an explicit --socket or TROLLM_ADB_SERVER_SOCKET")
    if not 1 <= args.boot_timeout <= 300:
        parser.error("--boot-timeout must be between 1 and 300 seconds")
    if not args.apk.is_file():
        parser.error("--apk must name an existing APK")
    result = None
    try:
        result = smoke(args)
    except (SmokeError, zipfile.BadZipFile, ET.ParseError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        result = {"schemaVersion": 1, "status": "failed", "error": str(exc)}
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, indent=2) + "\n")
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
