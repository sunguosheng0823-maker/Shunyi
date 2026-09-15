#!/usr/bin/env python3
"""Fetch pinned desktop sources and build native codecs on the target operating system."""
import argparse
import os
from pathlib import Path
import platform
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RUSTDESK = "851d2df88cc8ef7a8368f74e8b2e7254861ee00a"
VCPKG = "9e593bb18ea69cc5095e012465dcd675a822ed0d"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--sources-only", action="store_true")
args = parser.parse_args()

def run(command, cwd=ROOT):
    subprocess.run([str(x) for x in command], cwd=cwd, check=True)

def checkout(url, revision, path, sparse=None):
    if not (path / ".git").exists():
        path.mkdir(parents=True, exist_ok=True)
        run(["git", "init", path])
        run(["git", "remote", "add", "origin", url], path)
        run(["git", "fetch", "--depth", "1", "origin", revision], path)
        if sparse:
            run(["git", "sparse-checkout", "set", *sparse], path)
        run(["git", "checkout", "--detach", revision], path)
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=path, text=True).strip()
    if actual != revision:
        raise SystemExit(f"Refusing to change existing checkout {path}: expected {revision}, found {actual}")

source = ROOT / "engines/rustdesk/vendor/rustdesk"
checkout("https://github.com/rustdesk/rustdesk.git", RUSTDESK, source, ["libs", "src", "res/vcpkg", "res/vcpkg-triplets"])
run(["git", "submodule", "update", "--init", "--depth", "1", "libs/hbb_common"], source)
if args.sources_only:
    raise SystemExit(0)
system = platform.system()
if system not in ("Windows", "Linux"):
    raise SystemExit("Use build_desktop_preview.py for macOS native dependencies.")
vcpkg = ROOT / ".build/vcpkg"
checkout("https://github.com/microsoft/vcpkg.git", VCPKG, vcpkg)
windows = system == "Windows"
bootstrap = vcpkg / ("bootstrap-vcpkg.bat" if windows else "bootstrap-vcpkg.sh")
run(["cmd", "/c", bootstrap, "-disableMetrics"] if windows else ["sh", bootstrap, "-disableMetrics"])
arch = "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "x64"
triplet = arch + ("-windows-static" if windows else "-linux")
run([vcpkg / ("vcpkg.exe" if windows else "vcpkg"), "install", "libvpx", "libyuv", "aom", "--triplet", triplet,
     f"--x-install-root={vcpkg / 'installed'}", f"--overlay-ports={source / 'res/vcpkg'}", f"--overlay-triplets={source / 'res/vcpkg-triplets'}", "--disable-metrics"])
