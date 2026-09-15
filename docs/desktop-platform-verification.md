# 0.2.0 远控预览的平台验收

记录日期：2026-09-16。测试对象为匿名设备模式：UniRC v3 认证与传输、RustDesk 屏幕采集、VP8 编解码和键鼠输入。账号登录未包含在本次实现中。

## 已完成的验证

| 平台 | 实际测试环境 | 结果 |
| --- | --- | --- |
| Linux x64 | GitHub 托管 Ubuntu 22.04，X11 / Xvfb，独立测试窗口 | 原生客户端窗口、终端协议、桌面采集与解码、鼠标点击、按键、断开时释放按键通过 |
| Windows x64 | GitHub 托管 Windows Server 2022，已登录桌面，独立测试窗口 | 原生客户端窗口、终端协议、桌面采集与解码、鼠标点击、按键、断开时释放按键通过 |
| macOS Apple Silicon | 本机预览应用 | 原生界面、录屏和真实画面解码通过；实际控制仍待辅助功能授权后的独立窗口验收 |

Windows 与 Linux 的已验收产物均对应提交 `076d1c1e4b803cdec6b8de41fe368bf9817151f3`，两个原生任务均通过，记录见 [原生构建与控制验收](https://github.com/sunguosheng0823-maker/Shunyi/actions/runs/34998594892)；[常规代码与终端检查](https://github.com/sunguosheng0823-maker/Shunyi/actions/runs/34998594763)也通过。

| 该次原生测试观测值 | Windows | Linux |
| --- | --- | --- |
| 首帧分辨率 | 1024 × 768 | 1280 × 720 |
| 首帧耗时 | 145 ms | 90 ms |
| 已解码画面 | 首帧及输入后的更新画面，共 2 帧 | 首帧及输入后的更新画面，共 2 帧 |
| 可见客户端窗口启动 | 265 ms | 531 ms |
| 下载后 SHA-256 校验 | 23 个文件通过 | 19 个文件通过 |
| 点击、按键、断开释放 | 全部通过 | 全部通过 |

以上数据来自该次托管测试机与本机回环连接，不代表跨网络时延、持续帧率或用户电脑性能。Windows 的 DXGI 采集在这类桌面环境中可能于首帧之后返回 `invalid data`；适配层现按上游 RustDesk 的方式转用 GDI，恢复后的画面与键鼠控制已纳入验收。Windows 的 ConPTY 游标继承握手卡顿也已修复，并由真实 PowerShell 打开、输出、调整尺寸和关闭的测试覆盖。

## 验收方法

安装包先经过 `scripts/verify_desktop_artifacts.py` 校验：核对 `BUILD.json` 中全部文件的 SHA-256，检查原生二进制架构、便携包内文件一致性、Windows 运行库导入及 Linux Debian 包信息。原生窗口通过独立的 `scripts/verify_desktop_launch.py` 检查。

桌面验收使用临时 Agent、中继、设备凭据和独立 Tk 窗口。控制端实际建立 UniRC TLS 会话，接收真实屏幕数据并用 RustDesk 引擎解码 VP8，然后把鼠标和按键事件发送到该窗口。窗口将事件记录为 JSON，并更新可见内容。验收要求输入后仍能解码更新画面，主动断开后被控端释放尚未松开的按键。

自动键鼠脚本只允许在 Windows / Linux 的 CI 环境运行；临时凭据不写入公开验证附件。安装包、源码包和验收记录各自保留，编译成功与实际控制通过分别判断。

## 使用范围

此版本是预览版。Windows 10 / 11 用户真机、Linux 实体图形桌面、跨设备与跨网连接、长时间远控仍须另行验收。Linux 被控端当前要求 X11；Windows UAC 安全桌面、登录前控制及无人值守服务没有纳入本次交付。安装步骤见 [Windows 与 Linux 说明](desktop-cross-platform.md)，功能范围见 [远控预览说明](desktop-preview.md)。
