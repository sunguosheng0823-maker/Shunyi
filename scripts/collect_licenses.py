#!/usr/bin/env python3
"""Collect dependency license texts alongside binary distributions."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--out", required=True, type=Path)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=True)
target = "aarch64-apple-darwin" if platform.machine() == "arm64" else "x86_64-apple-darwin"
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target], cwd=root))
index = []
supplemental = {(p['name'], p['version']): p for p in json.loads((root / 'licenses/UPSTREAM-NOTICES.json').read_text())}
def collect(name, version, folder, license_expression, source=None):
    destination = args.out / (name.replace("/", "-") + "-" + version)
    copied = []
    for path in folder.rglob("*"):
        if not path.is_file() or path.is_symlink() or any(p in {"target", "node_modules", ".git"} for p in path.relative_to(folder).parts):
            continue
        if not path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")):
            continue
        relative = path.relative_to(folder)
        (destination / relative).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination / relative)
        copied.append(str(relative))
    extra = supplemental.get((name, version))
    if extra:
        for item in extra['texts']:
            path = root / item['path']
            if hashlib.sha256(path.read_bytes()).hexdigest() != item['sha256']:
                raise SystemExit(f"Upstream license digest mismatch: {path}")
            relative = Path('upstream') / path.name
            (destination / relative).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination / relative)
            copied.append(str(relative))
        (destination / 'UPSTREAM-NOTICES.json').write_text(json.dumps(extra, ensure_ascii=False, indent=2))
    index.append({"name": name, "version": version, "license": license_expression, "texts": copied, "source": source})

for package in metadata["packages"]:
    if package["source"] is not None:
        folder = Path(package["manifest_path"]).parent
        collect(package["name"], package["version"], folder, package.get("license"),
                f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download")
        if "MPL" in (package.get("license") or ""):
            archive = folder.parents[2] / "cache" / folder.parent.name / f"{package['name']}-{package['version']}.crate"
            sources = args.out / "SOURCE-ARCHIVES"
            sources.mkdir(exist_ok=True)
            shutil.copyfile(archive, sources / archive.name)
for name in ["@tauri-apps/api", "@xterm/xterm", "@xterm/addon-fit"]:
    folder = root / "client/ui/node_modules" / name
    package = json.loads((folder / "package.json").read_text())
    collect(name, package["version"], folder, package.get("license"), f"https://www.npmjs.com/package/{name}/v/{package['version']}")
shutil.copyfile(root / "client/ui/src/vendor/LICENSE.highlight.js", args.out / "LICENSE.highlight.js")
(args.out / "DEPENDENCIES.json").write_text(json.dumps(index, ensure_ascii=False, indent=2))
print(json.dumps({"dependencies": len(index), "missing_license_text": [p["name"] for p in index if not p["texts"]]}))
if any(not p['texts'] for p in index):
    raise SystemExit("Missing third-party license text; distribution is incomplete.")
