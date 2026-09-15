#!/usr/bin/env python3
"""Create a reviewable source release from an explicit allowlist, including dirty code."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--out", type=Path, required=True)
args = parser.parse_args()
roots = ["Cargo.toml", "Cargo.lock", "README.md", "README.en.md", "CONTRIBUTING.md", "CHANGELOG.md", "LICENSE", "NOTICE", "SECURITY.md", "ENCRYPTION.md", "CONTEXT.md", ".gitignore",
         "crates", "client", "protocol", "deploy", "scripts", "licenses", ".github", "docs/device-access-modes.md",
         "docs/getting-started.md", "docs/faq.md", "docs/architecture.md"]
excluded = {"target", "node_modules", "dist", "gen", ".git", ".mimosa", ".mimocode", ".zcode", ".DS_Store", "__pycache__"}
private_extensions = {".shunyi-cert", ".pem", ".key", ".p12", ".env", ".test-secret", ".log"}
paths = []
for name in roots:
    item = root / name
    candidates = [item] if item.is_file() else item.rglob("*")
    for path in candidates:
        relative = path.relative_to(root)
        if any(p in excluded for p in relative.parts) or path.suffix in private_extensions or path.name == "identity.json":
            continue
        if path.is_symlink():
            raise SystemExit(f"Refusing symlink: {relative}")
        if path.is_file():
            paths.append(path)
paths = sorted(set(paths))
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
args.out.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED) as archive:
    for path in paths:
        archive.write(path, str(Path("shunyi") / path.relative_to(root)))
    archive.writestr("shunyi/SOURCE-MANIFEST.json", json.dumps(manifest, ensure_ascii=False, indent=2))
print(json.dumps({"archive": str(args.out.resolve()), "files": len(manifest), "bytes": args.out.stat().st_size,
                  "sha256": hashlib.sha256(args.out.read_bytes()).hexdigest()}, ensure_ascii=False))
