#!/usr/bin/env python3
"""Create a reviewable source release from an explicit allowlist, including dirty code."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--out", type=Path, required=True)
parser.add_argument("--desktop", action="store_true", help="Include desktop adapters and corresponding pinned upstream sources")
parser.add_argument("--omit-vendor", action="store_true", help="For a Git checkout only; CI fetches pinned upstream sources before producing a full source archive")
args = parser.parse_args()
roots = ["Cargo.toml", "Cargo.lock", "README.md", "README.en.md", "CONTRIBUTING.md", "CHANGELOG.md", "LICENSE", "NOTICE", "SECURITY.md", "ENCRYPTION.md", "CONTEXT.md", ".gitignore",
         "crates", "client", "protocol", "deploy", "scripts", "licenses", "vendor/portable-pty", ".github", "docs/device-access-modes.md",
         "docs/getting-started.md", "docs/faq.md", "docs/architecture.md"]
if args.desktop:
    roots += ["engines", "docs/desktop-preview.md", "docs/desktop-cross-platform.md", "docs/desktop-platform-verification.md", "docs/windows.md", "protocol/UniRC-T-v3-desktop-preview.md"]
excluded = {"target", "node_modules", "dist", "gen", ".git", ".build", "binaries", "desktop-resources", ".mimosa", ".mimocode", ".zcode", ".DS_Store", "__pycache__"}
private_extensions = {".shunyi-cert", ".pem", ".key", ".p12", ".env", ".test-secret", ".log"}
paths = []
for name in roots:
    item = root / name
    candidates = [item] if item.is_file() else item.rglob("*")
    for path in candidates:
        relative = path.relative_to(root)
        if relative.parts[:3] == ("engines", "rustdesk", "vendor"):
            continue
        if any(p in excluded for p in relative.parts) or path.suffix in private_extensions or path.name in {"identity.json", "tauri.desktop-runtime.conf.json"}:
            continue
        if path.is_symlink():
            raise SystemExit(f"Refusing symlink: {relative}")
        if path.is_file():
            paths.append(path)
paths = sorted(set(paths))
upstream = []
if args.desktop and not args.omit_vendor:
    vendor = root / "engines/rustdesk/vendor/rustdesk"
    if not (vendor / "libs/hbb_common/.git").exists():
        raise SystemExit("Run scripts/prepare_desktop_deps.py --sources-only before packaging corresponding source")
    for checkout in (vendor, vendor / "libs/hbb_common"):
        files = subprocess.check_output(["git", "ls-files", "-z"], cwd=checkout).decode().split("\0")
        for name in files:
            path = checkout / name
            if name and path.is_file() and not path.is_symlink() and not any(part in {".git", "target", "node_modules"} for part in Path(name).parts):
                upstream.append((path, path.relative_to(root)))
manifest = []
for path in paths:
    data = path.read_bytes()
    if re.search(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----", data):
        raise SystemExit(f"Private-key material found: {path.relative_to(root)}")
    if re.search(rb"/(?:Users|Volumes)/", data) and path.suffix in {".md", ".json", ".js", ".rs", ".py"}:
        # Source examples may deliberately name a placeholder HOME path. Reject personal build evidence.
        text = data.decode("utf-8", errors="replace")
        if ("/Users/" + "mac/") in text or ("/Volumes/" + "KIOXIA/") in text:
            raise SystemExit(f"Personal workspace path found: {path.relative_to(root)}")
    manifest.append({"path": str(path.relative_to(root)), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
for path, relative in upstream:
    data = path.read_bytes()
    manifest.append({"path": str(relative), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "origin": "pinned-public-rustdesk-source"})
args.out.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED) as archive:
    for path in paths:
        archive.write(path, str(Path("shunyi") / path.relative_to(root)))
    for path, relative in upstream:
        archive.write(path, str(Path("shunyi") / relative))
    archive.writestr("shunyi/SOURCE-MANIFEST.json", json.dumps(manifest, ensure_ascii=False, indent=2))
print(json.dumps({"archive": str(args.out.resolve()), "files": len(manifest), "bytes": args.out.stat().st_size,
                  "sha256": hashlib.sha256(args.out.read_bytes()).hexdigest()}, ensure_ascii=False))
