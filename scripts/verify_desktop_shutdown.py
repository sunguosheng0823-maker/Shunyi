#!/usr/bin/env python3
"""Read-only native capture check: disconnect while the frame output pipe is blocked."""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("engine", type=Path)
parser.add_argument("receipt", type=Path)
args = parser.parse_args()
engine = args.engine.resolve()
child = subprocess.Popen([str(engine), "capture"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

def exact(size):
    data = b""
    while len(data) < size:
        part = child.stdout.read(size - len(data))
        if not part:
            raise RuntimeError("Engine ended before a complete packet")
        data += part
    return data

def header():
    total, size = struct.unpack(">II", exact(8))
    return json.loads(exact(size)), total - 4 - size

try:
    start = json.dumps({"kind": "start", "display": 0, "control": False}).encode()
    child.stdin.write(struct.pack(">II", 4 + len(start), len(start)) + start)
    child.stdin.flush()
    ready, body = header()
    assert ready["kind"] == "ready" and body == 0, ready
    frame, body = header()
    assert frame["kind"] == "frame", frame
    assert body > 32768, "Frame too small to establish macOS pipe backpressure"
    # Deliberately do not consume the encoded screen bytes. No screen content is saved.
    time.sleep(0.7)
    assert child.poll() is None
    started = time.monotonic()
    child.stdin.close()
    code = child.wait(timeout=2)
    receipt = {"engine": str(engine), "read_only": True, "unread_frame_bytes": body,
               "exit_code": code, "stdin_close_to_exit_ms": round((time.monotonic() - started) * 1000), "passed": code == 0}
    args.receipt.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(receipt, ensure_ascii=False))
    assert receipt["passed"]
finally:
    if child.poll() is None:
        child.kill()
        child.wait()
