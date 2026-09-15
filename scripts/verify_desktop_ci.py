#!/usr/bin/env python3
"""Real TLS -> capture -> VP8 -> decode -> input acceptance on an isolated CI desktop only."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("engine", type=Path)
args = parser.parse_args()
if os.environ.get("CI") != "true" or sys.platform not in ("win32", "linux"):
    raise SystemExit("Use only on an isolated Windows/Linux CI desktop; local foreground tests need user consent.")
suffix = ".exe" if sys.platform == "win32" else ""
env = dict(os.environ, SHUNYI_DESKTOP_ENGINE=str(args.engine.resolve()))
output = ROOT / "target/desktop-verification"
output.mkdir(parents=True, exist_ok=True)
children = []
temp_dir = tempfile.TemporaryDirectory(prefix="shunyi-desktop-ci-")
log = None
receipt = {"platform": sys.platform, "isolated_ci_desktop": True, "passed": False}
try:
    directory = temp_dir.name
    folder = Path(directory)
    log = (folder / "fixture.log").open("w")
    server = subprocess.Popen([str(ROOT / "target/debug/examples" / ("native_fixture" + suffix)), directory], env=env, stdout=log, stderr=log)
    children.append(server)
    deadline = time.monotonic() + 15
    while not (folder / "fixture.json").exists() and time.monotonic() < deadline:
        if server.poll() is not None: raise RuntimeError("Disposable device failed to start")
        time.sleep(0.1)
    window = subprocess.Popen([sys.executable, str(ROOT / "scripts/desktop_control_fixture.py"), directory], env=env, stdout=log, stderr=log)
    children.append(window)
    deadline = time.monotonic() + 12
    while not (folder / "control-target.json").exists() and time.monotonic() < deadline:
        if window.poll() is not None: raise RuntimeError("Disposable desktop window failed to start")
        time.sleep(0.1)
    target = json.loads((folder / "control-target.json").read_text())
    if target["pid"] != window.pid or target["front_pid"] != window.pid:
        raise RuntimeError("Test window did not acquire focus; no input was sent")
    result = subprocess.run([str(ROOT / "target/debug/examples" / ("desktop_acceptance" + suffix)), directory, "--input"], env=env, capture_output=True, text=True, timeout=40)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "Desktop acceptance failed")
    receipt["desktop"] = json.loads((folder / "desktop-control.json").read_text())
    time.sleep(2.2)
    events = [json.loads(line) for line in (folder / "control-events.jsonl").read_text().splitlines()]
    expected = [("mouse_down", 0), ("mouse_up", 0), ("key_down", 0), ("key_up", 0), ("key_down", 11), ("key_up", 11)]
    receipt["checks"] = {f"{kind}_{code}": any(e["event"] == kind and e["code"] == code for e in events) for kind, code in expected}
    receipt["event_count"] = len(events)
    receipt["events"] = events
    receipt["checks"]["b_released_after_disconnect"] = any(e["event"] == "key_up" and e["code"] == 11 and e["time"] >= receipt["desktop"]["disconnect_at"] - 0.02 for e in events)
    receipt["passed"] = all(receipt["checks"].values())
    for child in reversed(children):
        if child.poll() is None: child.terminate()
        child.wait(timeout=5)
    log.close()
except Exception as error:
    receipt["error"] = str(error)
finally:
    for child in reversed(children):
        if child.poll() is None:
            child.kill()
            child.wait()
    if log is not None: log.close()
    temp_dir.cleanup()
    (output / "native-desktop.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(receipt, ensure_ascii=False))
if not receipt["passed"]: raise SystemExit(1)
