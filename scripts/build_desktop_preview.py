#!/usr/bin/env python3
"""Build the optional AGPL macOS preview; never replaces /Applications/瞬移.app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RUSTDESK = "851d2df88cc8ef7a8368f74e8b2e7254861ee00a"
LIBYUV = "2dd4257364d39c38d79465c4ddc4b93137fe729b"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--debug", action="store_true", help="Build faster with debug symbols")
parser.add_argument("--package-only", action="store_true", help="Package already-built binaries; caller must build changed sources first")
parser.add_argument("--engine-target", type=Path, default=ROOT / "target/desktop-engine")
args = parser.parse_args()
if platform.system() != "Darwin":
    raise SystemExit("This preview packaging script currently supports macOS only.")
env = dict(os.environ)
env.pop("_", None)
for key in list(env):
    try:
        key.encode("utf-8"); env[key].encode("utf-8")
    except UnicodeEncodeError:
        env.pop(key)

def run(command, cwd=ROOT):
    subprocess.run([str(v) for v in command], cwd=cwd, env=env, check=True)

def read(command, cwd=ROOT):
    return subprocess.check_output([str(v) for v in command], cwd=cwd, env=env, text=True).strip()

def checkout(url, revision, destination, sparse=None):
    if not (destination / ".git").exists():
        destination.mkdir(parents=True, exist_ok=True)
        run(["git", "init", destination])
        run(["git", "remote", "add", "origin", url], destination)
        run(["git", "fetch", "--depth", "1", "origin", revision], destination)
        if sparse:
            run(["git", "sparse-checkout", "set", *sparse], destination)
        run(["git", "checkout", "--detach", revision], destination)
    if read(["git", "rev-parse", "HEAD"], destination) != revision:
        raise SystemExit(f"Refusing to change an existing checkout at {destination}; expected {revision}")

source = ROOT / "engines/rustdesk/vendor/rustdesk"
checkout("https://github.com/rustdesk/rustdesk.git", RUSTDESK, source, ["libs", "src"])
run(["git", "submodule", "update", "--init", "--depth", "1", "libs/hbb_common"], source)
brew = shutil.which("brew")
if not brew:
    raise SystemExit("Install Homebrew and run: brew install libvpx aom pkgconf cmake")
native = {}
for package in ("libvpx", "aom", "libvmaf"):
    native[package] = Path(read([brew, "--prefix", package]))
    if not native[package].is_dir():
        raise SystemExit(f"Missing {package}. Run: brew install libvpx aom pkgconf cmake")
cache = Path.home() / ".cache/shunyi/desktop-deps"
triplet = "arm64-osx" if platform.machine() == "arm64" else "x64-osx"
prefix = cache / "installed" / triplet
(prefix / "lib").mkdir(parents=True, exist_ok=True)
(prefix / "include").mkdir(parents=True, exist_ok=True)
yuv = cache / f"libyuv-{LIBYUV[:12]}"
checkout("https://chromium.googlesource.com/libyuv/libyuv", LIBYUV, yuv)
env["MACOSX_DEPLOYMENT_TARGET"] = "14.0"
run(["cmake", "-S", yuv, "-B", yuv / "build", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_OSX_DEPLOYMENT_TARGET=14.0", "-DBUILD_SHARED_LIBS=OFF", "-DUNIT_TEST=OFF"])
run(["cmake", "--build", yuv / "build", "--target", "yuv", "--parallel", "4"])
shutil.copy2(yuv / "build/libyuv.a", prefix / "lib/libyuv.a")
links = {
    prefix / "include/libyuv": yuv / "include/libyuv",
    prefix / "include/libyuv.h": yuv / "include/libyuv.h",
    prefix / "include/vpx": native["libvpx"] / "include/vpx",
    prefix / "include/aom": native["aom"] / "include/aom",
    prefix / "lib/libvpx.a": native["libvpx"] / "lib/libvpx.a",
    prefix / "lib/libaom.a": native["aom"] / "lib/libaom.a",
}
for target, original in links.items():
    if target.is_symlink():
        target.unlink()
    elif target.exists():
        raise SystemExit(f"Refusing to overwrite a regular file: {target}")
    target.symlink_to(original)
env.update(VCPKG_ROOT=str(cache), VCPKG_INSTALLED_ROOT=str(cache / "installed"), SHUNYI_VMAF_LIB_DIR=str(native["libvmaf"] / "lib"), CARGO_NET_GIT_FETCH_WITH_CLI="true")
profile = [] if args.debug else ["--release"]
profile_dir = "debug" if args.debug else "release"
env["CARGO_TARGET_DIR"] = str(args.engine_target.resolve())
if not args.package_only:
    run(["cargo", "build", "--locked", *profile], ROOT / "engines/rustdesk")
engine = args.engine_target.resolve() / profile_dir / "shunyi-desktop-engine"
run([engine, "self-test"])
env.pop("CARGO_TARGET_DIR", None)
if not args.package_only:
    run(["npm", "--prefix", ROOT / "client/ui", "run", "build"])
env["TAURI_CONFIG"] = json.dumps({"productName": "瞬移远控预览", "version": "0.2.0", "identifier": "cn.wkkj.shunyi.desktop-preview", "app": {"windows": [{"label": "main", "title": "瞬移远控预览", "width": 1280, "height": 820, "minWidth": 940, "minHeight": 600}]}})
if not args.package_only:
    run(["cargo", "build", "--locked", *profile, "-p", "unirc-client", "--features", "tauri/custom-protocol,desktop-preview"])
    run(["cargo", "build", "--locked", *profile, "-p", "rc-agent", "-p", "rc-server"])
stage = ROOT / "target/desktop-preview"
app = stage / "瞬移远控预览.app"
(app / "Contents/MacOS").mkdir(parents=True, exist_ok=True)
(app / "Contents/Resources").mkdir(exist_ok=True)
for service in ("rc-agent", "rc-server"):
    shutil.copy2(ROOT / "target" / profile_dir / service, stage / service)
# Keep the executable ASCII: this host's CFBundle/Wry lookup crashes after a Chinese rename.
shutil.copy2(ROOT / "target" / profile_dir / "unirc-client", app / "Contents/MacOS/unirc-client")
shutil.copy2(engine, app / "Contents/MacOS/shunyi-desktop-engine")
# Homebrew libaom can retain a VMAF dylib load command even alongside static linking.
# Bundle it explicitly so the preview does not depend on the build machine's Homebrew.
packaged_engine = app / "Contents/MacOS/shunyi-desktop-engine"
vmaf_load = str(native["libvmaf"] / "lib/libvmaf.3.dylib")
if vmaf_load in read(["otool", "-L", packaged_engine]):
    frameworks = app / "Contents/Frameworks"
    frameworks.mkdir(exist_ok=True)
    bundled_vmaf = frameworks / "libvmaf.3.dylib"
    shutil.copy2(vmaf_load, bundled_vmaf)
    bundled_vmaf.chmod(0o755)
    run(["install_name_tool", "-id", "@rpath/libvmaf.3.dylib", bundled_vmaf])
    run(["install_name_tool", "-change", vmaf_load, "@executable_path/../Frameworks/libvmaf.3.dylib", packaged_engine])
    shutil.copy2(native["libvmaf"] / "LICENSE", app / "Contents/Resources/VMAF-LICENSE.txt")
for binary in (app / "Contents/MacOS/unirc-client", packaged_engine, *list((app / "Contents/Frameworks").glob("*.dylib"))):
    for line in read(["otool", "-L", binary]).splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        if dependency.startswith("/") and not dependency.startswith(("/System/Library/", "/usr/lib/")):
            raise SystemExit(f"Unbundled native dependency in {binary.name}: {dependency}")
shutil.copy2(ROOT / "client/src-tauri/icons/icon.icns", app / "Contents/Resources/icon.icns")
shutil.copy2(ROOT / "engines/rustdesk/LICENSE", app / "Contents/Resources/AGPL-3.0.txt")
shutil.copy2(ROOT / "engines/rustdesk/NOTICE.md", app / "Contents/Resources/DESKTOP-NOTICE.md")
info = {"CFBundleExecutable": "unirc-client", "CFBundleIdentifier": "cn.wkkj.shunyi.desktop-preview", "CFBundleName": "瞬移远控预览", "CFBundleDisplayName": "瞬移远控预览", "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "0.2.0", "CFBundleVersion": "1", "CFBundleIconFile": "icon.icns", "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "14.0", "NSPrincipalClass": "NSApplication", "NSScreenCaptureUsageDescription": "远程桌面共享需要录制当前屏幕。"}
(app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
run(["codesign", "--force", "--deep", "--sign", "-", app])
run(["codesign", "--verify", "--deep", "--strict", app])
run([packaged_engine, "self-test"])
receipt = {"app": str(app), "profile": profile_dir, "rustdesk_revision": RUSTDESK, "libyuv_revision": LIBYUV, "engine_sha256": hashlib.sha256(engine.read_bytes()).hexdigest(), "client_sha256": hashlib.sha256((app / "Contents/MacOS/unirc-client").read_bytes()).hexdigest(), "brew_versions": {p: read([brew, "list", "--versions", p]) for p in native}, "distribution": "local preview, ad-hoc signed; not notarized; Windows and older macOS unverified"}
receipt["engine_sha256"] = hashlib.sha256(packaged_engine.read_bytes()).hexdigest()
receipt["services_sha256"] = {s: hashlib.sha256((stage / s).read_bytes()).hexdigest() for s in ("rc-agent", "rc-server")}
receipt["native_dependencies_bundled"] = True
(stage / "build.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(receipt, ensure_ascii=False))
