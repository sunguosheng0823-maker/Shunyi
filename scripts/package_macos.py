#!/usr/bin/env python3
"""Package the tested native build and standalone services with dependency notices."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
if platform.system() != "Darwin":
    raise SystemExit("macOS packaging requires macOS.")
architecture = "arm64" if platform.machine() == "arm64" else "x86_64"
release = root / "release"
name = f"Shunyi-0.1.0-macOS-{architecture}"
stage = release / name
app = stage / "瞬移.app"
stage.mkdir(parents=True, exist_ok=True)
for binary in ("rc-agent", "rc-server"):
    shutil.copy2(root / "target/release" / binary, stage / binary)
subprocess.run(["python3", "scripts/collect_licenses.py", "--out", str(release / "THIRD-PARTY-LICENSES")], cwd=root, check=True)
subprocess.run(["ditto", str(root / "target/release/bundle/macos/瞬移.app"), str(app)], check=True)
notices = app / "Contents/Resources/THIRD-PARTY-LICENSES"
shutil.copytree(release / "THIRD-PARTY-LICENSES", notices, dirs_exist_ok=True)
for filename in ("LICENSE", "NOTICE", "README.md", "README.en.md", "SECURITY.md", "CHANGELOG.md", "CONTRIBUTING.md"):
    shutil.copy2(root / filename, stage / filename)
for filename in ("getting-started.md", "faq.md", "architecture.md", "device-access-modes.md"):
    (stage / "docs").mkdir(exist_ok=True)
    shutil.copy2(root / "docs" / filename, stage / "docs" / filename)
shutil.copytree(root / "protocol", stage / "protocol", dirs_exist_ok=True)
shutil.copytree(root / "deploy", stage / "deploy", dirs_exist_ok=True)
(stage / "安装说明.txt").write_text("""瞬移 0.1.0 · 免账号远程终端

将 瞬移.app 拖入“应用程序”后打开。现有用户请先结束需要保留的终端任务，再退出旧版并打开新版。
这是本地开发签名构建，尚未进行 Apple Developer ID 签名和公证。

设置中填写自己的中继地址。在被控端的“本机接入与凭据”中开始共享，
导出连接证书或生成临时密码；主控端新建主机并选择“瞬移协议”。
无账号业务、无公共设备发现服务。本机共享默认关闭，终端使用 Agent 的系统用户权限。

rc-agent 和 rc-server 是独立命令行程序。持续后台运行方法见 deploy/README.md。
图形客户端、Agent 和中继的开源说明见 README.md，凭据边界见 SECURITY.md。
依赖许可及源代码获取地址位于 瞬移.app/Contents/Resources/THIRD-PARTY-LICENSES/。
""", encoding="utf-8")
subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
zip_path = release / (name + ".zip")
zip_path.unlink(missing_ok=True)
subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(stage), str(zip_path)], check=True)
print(json.dumps({"archive": str(zip_path), "sha256": hashlib.sha256(zip_path.read_bytes()).hexdigest(),
                  "app": str(app), "bytes": zip_path.stat().st_size}, ensure_ascii=False))
