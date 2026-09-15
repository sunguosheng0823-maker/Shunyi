# 瞬移远控预览：Windows 与 Linux

Windows 和 Linux 预览版与 macOS 使用相同的 UniRC v3 设备认证、单次临时密码、桌面数据传输和终端工作区。账号登录仍未启用。应用名称、应用标识和设备凭据目录与 0.1 正式终端版分开。

## 平台范围

| 系统 | 安装格式 | 当前会话要求 |
| --- | --- | --- |
| Windows 10 1809 及以上、Windows 11，x64 | NSIS `.exe`、MSI、便携 ZIP | 已登录的普通桌面；不包含 UAC 安全桌面、登录前控制或后台系统服务 |
| Ubuntu 22.04 及更新版本，x64 | `.deb`、AppImage、便携 ZIP | 被控端须为已登录的 X11/Xorg 桌面；Wayland 被控端会明确拒绝启动采集 |

便携 ZIP 内的客户端与 `shunyi-desktop-engine` 必须放在同一目录。Linux 便携版还需要系统图形库，优先使用安装包。此预览没有商业代码签名；系统、CPU 和桌面环境的真机验收结果另行记录，构建完成不等于所有平台场景可用。

被控端在「设置 → 本机接入与凭据」选择仅查看或允许控制，填写兼容 v3 的中继地址后开始共享。控制端添加对方的设备 ID 和证书/临时密码，通过主机行的屏幕按钮查看，或右键主机选择控制。控制端与被控端的操作系统可以不同。默认关闭共享；不会自动提权或安装常驻服务。

## 安装与启动

Windows 优先使用 `.exe` 安装器，MSI 作为另一种安装格式提供。安装器支持中文。便携 ZIP 解压后运行 `shunyi-desktop-preview.exe`，保留同目录的 `shunyi-desktop-engine.exe` 和许可文件。便携客户端需要系统已有 WebView2 Runtime；安装器会检查 WebView2。

Ubuntu 安装 DEB 时让 apt 同时处理系统依赖：

```bash
sudo apt install ./shunyi-desktop-preview_0.2.0_amd64.deb
```

安装后的菜单名称是「瞬移远控预览」，内部包名与命令名是 `shunyi-desktop-preview`。AppImage 可单独运行：

```bash
chmod +x shunyi-desktop-preview_0.2.0_amd64.AppImage
./shunyi-desktop-preview_0.2.0_amd64.AppImage
```

如果系统没有 FUSE，可用 `./shunyi-desktop-preview_0.2.0_amd64.AppImage --appimage-extract-and-run` 启动。

被控 Linux 设备应在登录界面选择 Xorg/X11 会话。预览包不会更改显示服务器或系统权限。自建中继使用同包 `rc-server` 的 v3 版本，0.1 中继只支持终端；完整连接步骤见 [远控预览说明](desktop-preview.md)。

## 从源码构建

在目标系统安装 Rust stable、Node.js 22、Python 3、Git 与原生 C/C++ 工具链。Windows 使用 Visual Studio 2022 C++ 开发环境、LLVM、NASM、Protobuf；Linux 使用 `.github/workflows/desktop-preview-build.yml` 中列出的开发包。

```bash
npm --prefix client/ui ci
python scripts/build_desktop_cross.py
```

脚本下载固定提交的 RustDesk 和 vcpkg，构建引擎并执行真实 VP8 编解码自测，再生成安装器、独立 Agent、中继、许可和 `BUILD.json` 校验清单。输出目录为 `target/desktop-distribution/<Rust target>/`。已准备好依赖时可使用 `--skip-deps`。

GitHub Actions 在原生 Windows 与 Ubuntu 构建机上执行同样流程，产物作为该次运行的附件提供。CI 的“通过”表示实际编译、打包和已列出的检查通过；用户真机、跨网、长时间远控验收须另行完成。

## 源码与许可

组合远控预览按 AGPL-3.0-only 提供；原有终端代码仍保留其 Apache-2.0 许可。`engines/rustdesk/NOTICE.md` 记录引擎来源和组合发行要求。`scripts/package_source.py --desktop` 会包含引擎适配源码、固定版本获取脚本和对应上游库源码，并排除本机凭据、日志、测试图片及编译目录。

构建依据：[RustDesk Windows 构建说明](https://rustdesk.com/docs/en/dev/build/windows/)、[RustDesk Linux 构建说明](https://rustdesk.com/docs/en/dev/build/linux/)、[Tauri 外部引擎打包](https://v2.tauri.app/develop/sidecar/)、[Tauri Linux 分发约束](https://v2.tauri.app/distribute/appimage/)。
