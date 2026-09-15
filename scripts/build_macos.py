#!/usr/bin/env python3
"""Build the native app without leaking malformed environment values into Cargo."""
import os
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
env = dict(os.environ)
env.pop("_", None)
for key in list(env):
    try:
        key.encode("utf-8")
        env[key].encode("utf-8")
    except UnicodeEncodeError:
        del env[key]
if sys.platform != "darwin":
    raise SystemExit("This build script requires macOS.")
cli = root / "client/ui/node_modules/.bin/tauri"
if not cli.exists():
    raise SystemExit("Run npm --prefix client/ui ci first.")
subprocess.run(["cargo", "build", "--locked", "--release", "-p", "rc-agent", "-p", "rc-server"], cwd=root, env=env, check=True)
subprocess.run([str(cli), "build", "--bundles", "app", "--", "--locked"], cwd=root / "client", env=env, check=True)
print(root / "target/release/bundle/macos/瞬移.app")
