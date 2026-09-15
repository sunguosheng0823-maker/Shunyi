#!/usr/bin/env python3
"""Verify a disposable native_fixture. --input takes foreground focus; obtain user consent first."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import time

REPO = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("fixture", type=Path)
parser.add_argument("--engine", required=True, type=Path)
parser.add_argument("--input", action="store_true", help="Use only after explicit approval to take foreground focus and send input")
args = parser.parse_args()
root = args.fixture.resolve()
env = dict(os.environ, SHUNYI_DESKTOP_ENGINE=str(args.engine.resolve()))
env.pop("_", None)
receiver = None
try:
    if args.input:
        binary = root / "control-fixture"
        subprocess.run(["swiftc", str(REPO / "scripts/desktop_control_fixture.swift"), "-o", str(binary)], check=True)
        target = root / "control-target.json"
        target.unlink(missing_ok=True)
        receiver = subprocess.Popen([str(binary), str(root)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 8
        while not target.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        coordinates = json.loads(target.read_text())
        if coordinates["pid"] != receiver.pid or coordinates["front_pid"] != receiver.pid:
            raise RuntimeError("Independent test window is not in the foreground; refusing input.")
    command = [str(REPO / "target/debug/examples/desktop_acceptance"), str(root)]
    if args.input:
        command.append("--input")
    result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=40)
    if result.stdout:
        print(result.stdout.strip())
    if args.input:
        # Allow the capture helper to process stdin EOF and release held input.
        time.sleep(2.2)
        events = [json.loads(line) for line in (root / "control-events.jsonl").read_text().splitlines()]
        def received(kind, code=0):
            return any(e["event"] == kind and e["code"] == code for e in events)
        checks = {"mouse_down": received("mouse_down"), "mouse_up": received("mouse_up"),
                  "key_a_down": received("key_down", 0), "key_a_up": received("key_up", 0),
                  "key_b_down": received("key_down", 11), "key_b_released_on_disconnect": received("key_up", 11)}
        receipt = {"user_approved_independent_window": True, "test_process_id": receiver.pid,
                   "event_count": len(events), "checks": checks,
                   "passed": result.returncode == 0 and all(checks.values()),
                   "error": result.stderr.strip() if result.returncode else None}
        (root / "input-verification.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(receipt, ensure_ascii=False))
        if not receipt["passed"]:
            raise SystemExit(1)
    if result.returncode:
        raise SystemExit(result.stderr.strip())
finally:
    if receiver is not None and receiver.poll() is None:
        receiver.terminate()
        try:
            receiver.wait(timeout=3)
        except subprocess.TimeoutExpired:
            receiver.kill()
            receiver.wait()
