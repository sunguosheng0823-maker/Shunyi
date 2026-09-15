# 0.2.0 远控预览的平台验收

记录日期：2026-09-16。测试对象为匿名设备模式：UniRC v3 认证与传输、RustDesk 屏幕采集、VP8 编解码和键鼠输入。账号登录未包含在本次实现中。

## 已完成的验证

| 平台 | 实际测试环境 | 结果 |
| --- | --- | --- |
| Linux x64 | GitHub 托管 Ubuntu 22.04，X11 / Xvfb，独立测试窗口 | 原生客户端窗口、终端协议、桌面采集与解码、鼠标点击、按键、断开时释放按键通过 |
| Windows x64 | GitHub 托管 Windows Server 2022，已登录桌面，独立测试窗口 | 安装包、原生窗口、终端协议已通过；桌面连续采集修复仍在验收 |
| macOS Apple Silicon | 本机预览应用 | 原生界面、录屏和真实画面解码通过；实际控制仍待辅助功能授权后的独立窗口验收 |

Linux 已验收产物对应提交 `968d5acbaa6093e125566e1721ff50981ae2fa8f`，原生构建记录见 [GitHub Actions](https://github.com/sunguosheng0823-maker/Shunyi/actions/runs/34991653019)。首帧为 1280 × 720，耗时 80 ms；客户端可见窗口启动耗时 283 ms。这些是该次托管测试机上的观测值，不代表跨网络时延或用户电脑性能。

## 验收方法

安装包先经过 `scripts/verify_desktop_artifacts.py` 校验：核对 `BUILD.json` 中全部文件的 SHA-256，检查原生二进制架构、便携包内文件一致性、Windows 运行库导入及 Linux Debian 包信息。原生窗口通过独立的 `scripts/verify_desktop_launch.py` 检查。

桌面验收使用临时 Agent、中继、设备凭据和独立 Tk 窗口。控制端实际建立 UniRC TLS 会话，接收真实屏幕数据并用 RustDesk 引擎解码 VP8，然后把鼠标和按键事件发送到该窗口。窗口将事件记录为 JSON，并更新可见内容。验收要求输入后仍能解码更新画面，主动断开后被控端释放尚未松开的按键。

自动键鼠脚本只允许在 Windows / Linux 的 CI 环境运行；临时凭据不写入公开验证附件。安装包、源码包和验收记录各自保留，编译成功与实际控制通过分别判断。

## 使用范围

此版本是预览版。Windows 10 / 11 用户真机、Linux 实体图形桌面、跨设备与跨网连接、长时间远控仍须另行验收。Linux 被控端当前要求 X11；Windows UAC 安全桌面、登录前控制及无人值守服务没有纳入本次交付。安装步骤见 [Windows 与 Linux 说明](desktop-cross-platform.md)，功能范围见 [远控预览说明](desktop-preview.md)。
