#!/usr/bin/env python3
"""Read-only checks of downloaded native packages and their portable payloads."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import struct
import tarfile
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
parser.add_argument("--receipt", type=Path)
args = parser.parse_args()

def pe_imports(data):
    assert data[:2] == b"MZ"
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe:pe + 4] == b"PE\0\0"
    machine, count = struct.unpack_from("<HH", data, pe + 4)
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    assert machine == 0x8664 and struct.unpack_from("<H", data, optional)[0] == 0x20B
    sections = [struct.unpack_from("<IIII", data, optional + optional_size + i * 40 + 8) for i in range(count)]
    def offset(rva):
        for virtual_size, virtual_address, raw_size, raw_offset in sections:
            if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
                return raw_offset + rva - virtual_address
        raise ValueError("PE RVA is outside image sections")
    imports_rva = struct.unpack_from("<I", data, optional + 112 + 8)[0]
    cursor = offset(imports_rva)
    names = []
    for _ in range(256):
        descriptor = struct.unpack_from("<IIIII", data, cursor)
        if not any(descriptor): return sorted(names)
        start = offset(descriptor[3]); end = data.index(b"\0", start, start + 260)
        names.append(data[start:end].decode("ascii"))
        cursor += 20
    raise ValueError("Too many PE imports")

def deb_payload(path):
    data = path.read_bytes()
    assert data.startswith(b"!<arch>\n")
    cursor = 8
    payload = {}
    while cursor + 60 <= len(data):
        header = data[cursor:cursor + 60]
        name = header[:16].decode().strip().rstrip("/")
        size = int(header[48:58].decode().strip())
        body = data[cursor + 60:cursor + 60 + size]
        if name.startswith(("data.tar", "control.tar")):
            with tarfile.open(fileobj=io.BytesIO(body), mode="r:*") as archive:
                for member in archive.getmembers():
                    key = member.name.lstrip("./")
                    if member.isfile() and (key.startswith("usr/bin/") or key == "control"):
                        payload[key] = archive.extractfile(member).read()
        cursor += 60 + size + size % 2
    assert "control" in payload and "usr/bin/shunyi-desktop-preview" in payload
    return payload

def normalize_bundle_marker(data, allowed):
    marker = b"__TAURI_BUNDLE_TYPE_VAR_"
    assert data.count(marker) == 1, "Expected one Tauri bundle type marker"
    index = data.index(marker) + len(marker)
    assert data[index:index + 3] in allowed, "Unexpected Tauri bundle type"
    return data[:index] + b"UNK" + data[index + 3:]

reports = []
manifests = list(args.directory.rglob("BUILD.json"))
assert manifests, "No native build manifests found"
for path in manifests:
    build = json.loads(path.read_text(encoding="utf-8")); root = path.parent.resolve()
    for relative, digest in build["files"].items():
        file = (root / relative).resolve()
        assert file.is_relative_to(root), "Manifest path escapes its build directory"
        assert hashlib.sha256(file.read_bytes()).hexdigest() == digest, relative
    windows = "windows" in build["target"]
    suffix = ".exe" if windows else ""
    binaries = ["shunyi-desktop-preview", "shunyi-desktop-engine", "rc-agent", "rc-server"]
    report = {"target": build["target"], "hashes_verified": len(build["files"]), "native_binaries": {}}
    for name in binaries:
        data = (root / "portable" / (name + suffix)).read_bytes()
        if windows:
            report["native_binaries"][name] = {"architecture": "x64", "imports": pe_imports(data)}
        else:
            assert data[:4] == b"\x7fELF" and data[4:6] == b"\x02\x01" and struct.unpack_from("<H", data, 18)[0] == 62
            report["native_binaries"][name] = {"architecture": "x64"}
    for archive in root.glob("*.zip"):
        with zipfile.ZipFile(archive) as zipped:
            for name in binaries:
                assert zipped.read("portable/" + name + suffix) == (root / "portable" / (name + suffix)).read_bytes()
    if not windows:
        for package in root.glob("*.deb"):
            payload = deb_payload(package)
            fields = dict(line.split(": ", 1) for line in payload["control"].decode().splitlines() if ": " in line and not line.startswith(" "))
            assert fields["Package"] == "shunyi-desktop-preview", "Invalid or conflicting Debian package name"
            assert fields["Architecture"] == "amd64"
            assert payload["usr/bin/shunyi-desktop-engine"] == (root / "portable/shunyi-desktop-engine").read_bytes()
            # Tauri changes exactly this three-byte marker for each installer format.
            assert normalize_bundle_marker(payload["usr/bin/shunyi-desktop-preview"], {b"DEB"}) == normalize_bundle_marker((root / "portable/shunyi-desktop-preview").read_bytes(), {b"UNK", b"APP", b"DEB"})
        report["deb_engine_matches_portable"] = True
        report["deb_client_matches_except_bundle_type_marker"] = True
        report["deb_package_name"] = "shunyi-desktop-preview"
    report["passed"] = True
    reports.append(report)
result = {"builds": reports, "note": "File and package checks are separate from native GUI and input acceptance."}
if args.receipt:
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps(result, ensure_ascii=False))
