#!/usr/bin/env python3
"""Synthetic transport fixture. It never captures a screen or injects OS input."""
import json
import struct
import sys

def read_exact(count):
    out = bytearray()
    while len(out) < count:
        part = sys.stdin.buffer.read(count - len(out))
        if not part:
            raise EOFError
        out.extend(part)
    return out

def receive():
    length, size = struct.unpack('>II', read_exact(8))
    header = json.loads(read_exact(size))
    read_exact(length - size - 4)
    return header

def send(header, data=b''):
    encoded = json.dumps(header).encode()
    sys.stdout.buffer.write(struct.pack('>II', 4 + len(encoded) + len(data), len(encoded)) + encoded + data)
    sys.stdout.buffer.flush()

try:
    assert receive()['kind'] == 'start'
    send({'kind': 'ready', 'display': {'width': 64, 'height': 64, 'origin_x': 0, 'origin_y': 0, 'scale': 1.0, 'cursor_embedded': False}})
    send({'kind': 'frame', 'sequence': 1, 'width': 64, 'height': 64}, b'frame-test-' * 60000)
    while receive()['kind'] != 'stop':
        pass
except (EOFError, BrokenPipeError):
    pass
