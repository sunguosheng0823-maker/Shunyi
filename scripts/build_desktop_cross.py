#!/usr/bin/env python3
"""Build Windows or Linux desktop preview installers using native tools, then write hashes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--skip-deps", action="store_true")
args = parser.parse_args()
system = platform.system()
if system not in ("Windows", "Linux"):
    raise SystemExit("Run on Windows or Linux; use build_desktop_preview.py for macOS.")
windows = system == "Windows"
suffix = ".exe" if windows else ""
env = dict(os.environ)
env.pop("_", None)
vcpkg = ROOT / ".build/vcpkg"
env.update(VCPKG_ROOT=str(vcpkg), VCPKG_INSTALLED_ROOT=str(vcpkg / "installed"), CARGO_NET_GIT_FETCH_WITH_CLI="true", CARGO_TARGET_DIR=str(ROOT / "target"))

def run(command, cwd=ROOT, overrides=None):
    subprocess.run([str(x) for x in command], cwd=cwd, env=env | (overrides or {}), check=True)

if not args.skip_deps:
    run([os.sys.executable, ROOT / "scripts/prepare_desktop_deps.py"])
target = subprocess.check_output(["rustc", "--print", "host-tuple"], text=True).strip()
engine_target = ROOT / "target/desktop-engine"
run(["cargo", "build", "--locked", "--release"], ROOT / "engines/rustdesk", {"CARGO_TARGET_DIR": str(engine_target)})
engine = engine_target / "release" / ("shunyi-desktop-engine" + suffix)
run([engine, "self-test"])
binaries = ROOT / "client/src-tauri/binaries"
binaries.mkdir(exist_ok=True)
shutil.copy2(engine, binaries / ("shunyi-desktop-engine-" + target + suffix))
resources = ROOT / "client/src-tauri/desktop-resources"
resources.mkdir(exist_ok=True)
for source, name in [(ROOT / "engines/rustdesk/LICENSE", "AGPL-3.0.txt"), (ROOT / "engines/rustdesk/NOTICE.md", "DESKTOP-NOTICE.md"), (ROOT / "LICENSE", "TERMINAL-LICENSE.txt")]:
    shutil.copy2(source, resources / name)
for copyright_file in (vcpkg / "installed").glob("*/share/*/copyright"):
    destination = resources / copyright_file.parent.name
    destination.mkdir(exist_ok=True)
    shutil.copy2(copyright_file, destination / "LICENSE.txt")
shutil.copy2(ROOT / "vendor/portable-pty/LICENSE.md", resources / "PORTABLE-PTY-MIT.txt")
run(["cargo", "build", "--locked", "--release", "-p", "rc-agent", "-p", "rc-server"])
config_name = "tauri.desktop-preview.conf.json"
if not windows:
    # RustDesk loads libxdo dynamically, so ELF dependency scanning cannot discover it.
    choices = list(Path("/usr/lib").glob("*/libxdo.so.3")) + list(Path("/usr/lib").glob("libxdo.so.3"))
    if not choices:
        raise SystemExit("Missing libxdo.so.3; install libxdo-dev before creating the AppImage")
    config = json.loads((ROOT / "client/src-tauri" / config_name).read_text(encoding="utf-8"))
    # Debian's package identifier must be ASCII; window and launcher labels stay localized.
    config["productName"] = "shunyi-desktop-preview"
    config["bundle"]["linux"]["deb"]["desktopTemplate"] = "desktop-preview.desktop.hbs"
    config["bundle"]["linux"]["appimage"] = {"files": {"/usr/lib/libxdo.so.3": str(choices[0].resolve())}}
    config_name = "tauri.desktop-runtime.conf.json"
    (ROOT / "client/src-tauri" / config_name).write_text(json.dumps(config, ensure_ascii=False, indent=2), encoding="utf-8")
run(["npm.cmd" if windows else "npm", "--prefix", ROOT / "client/ui", "run", "build"])
run(["node", ROOT / "client/ui/node_modules/@tauri-apps/cli/tauri.js", "build", "--verbose", "--config", "src-tauri/" + config_name, "--features", "desktop-preview", "--bundles", "nsis,msi" if windows else "deb,appimage"], ROOT / "client")
out = ROOT / "target/desktop-distribution" / target
out.mkdir(parents=True, exist_ok=True)
portable = out / "portable"
portable.mkdir(exist_ok=True)
main = ROOT / "target/release" / ("shunyi-desktop-preview" + suffix)
for source in (main, engine, ROOT / "target/release" / ("rc-agent" + suffix), ROOT / "target/release" / ("rc-server" + suffix)):
    shutil.copy2(source, portable / source.name)
shutil.copytree(resources, portable / "licenses", dirs_exist_ok=True)
shutil.copy2(ROOT / "docs/desktop-cross-platform.md", portable / "README.md")
patterns = ["nsis/*.exe", "msi/*.msi"] if windows else ["deb/*.deb", "appimage/*.AppImage"]
for pattern in patterns:
    packages = list((ROOT / "target/release/bundle").glob(pattern))
    if not packages:
        raise SystemExit(f"Installer missing: {pattern}")
    for package in packages:
        if package.suffix == ".deb":
            run(["dpkg-deb", "--info", package])
        shutil.copy2(package, out / package.name)
with zipfile.ZipFile(out / ("Shunyi-Desktop-Preview-0.2.0-" + target + ".zip"), "w", zipfile.ZIP_DEFLATED) as archive:
    for file in sorted(portable.rglob("*")):
        if file.is_file(): archive.write(file, file.relative_to(portable.parent))
manifest = {str(p.relative_to(out)): hashlib.sha256(p.read_bytes()).hexdigest() for p in out.rglob("*") if p.is_file() and p.name != "BUILD.json"}
(out / "BUILD.json").write_text(json.dumps({"target": target, "version": "0.2.0", "native_build": True, "files": manifest}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"output": str(out), "target": target, "files": len(manifest)}, ensure_ascii=False))
