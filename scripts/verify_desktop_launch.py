#!/usr/bin/env python3
"""Check the packaged client opens a visible native window in an isolated CI desktop."""

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import platform
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("client", type=Path)
parser.add_argument("--receipt", type=Path, required=True)
args = parser.parse_args()
system = platform.system()
if os.environ.get("CI") != "true" or system not in {"Windows", "Linux"}:
    raise SystemExit("Run only on an isolated Windows or Linux CI desktop")

def visible_windows(pid):
    if system == "Linux":
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(pid)], capture_output=True, text=True)
        titles = []
        for window in result.stdout.split():
            title = subprocess.run(["xdotool", "getwindowname", window], capture_output=True, text=True)
            if title.returncode == 0 and title.stdout.strip():
                titles.append(title.stdout.strip())
        return titles
    user32 = ctypes.windll.user32
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    titles = []
    def callback(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            title = ctypes.create_unicode_buffer(512)
            user32.GetWindowTextW(hwnd, title, len(title))
            if title.value:
                titles.append(title.value)
        return True
    user32.EnumWindows(callback_type(callback), 0)
    return titles

args.receipt.parent.mkdir(parents=True, exist_ok=True)
report = {"platform": system.lower(), "isolated_ci_desktop": True, "passed": False}
child = None
try:
    with args.receipt.with_suffix(".log").open("wb") as log:
        child = subprocess.Popen([str(args.client.resolve())], stdout=log, stderr=log)
        started = time.monotonic()
        while time.monotonic() - started < 25:
            if child.poll() is not None:
                raise RuntimeError(f"Client exited before opening a window: {child.returncode}")
            titles = visible_windows(child.pid)
            if any("瞬移" in title for title in titles):
                report.update(passed=True, visible_titles=titles, startup_ms=round((time.monotonic() - started) * 1000))
                break
            time.sleep(0.25)
        if not report["passed"]:
            raise RuntimeError("Packaged client did not open a visible Shunyi window")
except Exception as error:
    report["error"] = str(error)
finally:
    if child and child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
    args.receipt.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps(report, ensure_ascii=False))
raise SystemExit(0 if report["passed"] else 1)
