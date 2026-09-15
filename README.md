# 瞬移 · 免账号远程终端

瞬移 0.1 是以终端为主的 macOS 客户端：本地项目与文件预览、SSH、通过 UniRC-T 连接的远程终端，以及右侧快捷指令库。

当前版本为 **0.1.0 预发布版**，开源范围是 **无需账号的设备访问**：长期连接证书 + 单次临时密码 + 可自托管中继。账号注册、登录、同账号设备发现、云同步和远程桌面不在此版本中。

正在开发的 **0.2 远控预览版**已接入 RustDesk 采集、VP8 编解码和键鼠引擎，连接认证与中继继续使用 UniRC。独立预览包的构建、使用、许可范围与验收状态见 [远控预览说明](docs/desktop-preview.md)。

[English](README.en.md) · [快速开始](docs/getting-started.md) · [常见问题](docs/faq.md) · [架构](docs/architecture.md) · [版本记录](CHANGELOG.md) · [参与贡献](CONTRIBUTING.md)

## 安装与下载

[下载 0.1.0 预发布版](https://github.com/sunguosheng0823-maker/Shunyi/releases/tag/v0.1.0) · [CI 运行记录](https://github.com/sunguosheng0823-maker/Shunyi/actions/workflows/ci.yml)

在项目版本发布页下载 `Shunyi-0.1.0-macOS-arm64.zip`，解压后将 `瞬移.app` 放入“应用程序”。分发包同时包含 macOS 版独立 Agent、中继、部署模板及第三方许可文件。Linux 部署需要从源码构建对应平台二进制，不能直接运行 Mac 包中的程序。

当前包使用本地开发签名，尚未进行 Apple Developer ID 签名和公证。请核对发布页 `SHA256SUMS.txt`；也可按下文从源码构建。验证范围及已知限制见 [CHANGELOG.md](CHANGELOG.md)。

## 连接方式

| 方式 | 需要提供 | 使用规则 |
| --- | --- | --- |
| 连接证书 | 设备端导出的 `.shunyi-cert` 文件 | 可重复连接；轮换后旧证书不能建立新连接 |
| 临时密码 | 完整设备 ID + 临时密码 | 首次认证成功后仅授权该次访问；断开后不能重连 |
| SSH | 地址、用户名及密码或私钥 | 使用系统 `~/.ssh/known_hosts` 验证服务器身份 |

每台设备同时维护一份当前有效的长期证书和至多一个未使用的临时密码。证书包含私钥，只交给受信任的人。中继准入令牌不能代替设备认证。

## 在 Mac 中使用

1. 在“设置”中保存自托管中继地址，例如 `wss://relay.example.com`。只有中继配置了准入令牌时才需要填写令牌。
2. 被控 Mac 打开“设置 → 本机接入与凭据”，点击“开始共享”。软件自动初始化设备身份，可以导出连接证书或生成临时密码。
3. 主控 Mac 新建主机，选择“瞬移协议”。长期连接时选择证书文件，设备 ID 会自动填写；临时连接时输入对方设备 ID 和临时密码。
4. 终端以被控 Agent 的系统用户运行。普通用户的证书不会自动授予 root 权限。

本机共享默认关闭。图形客户端中的共享只在应用运行期间持续；持续无人值守访问请运行下方的独立 Agent 服务。切换项目、标签或折叠侧栏不会断开终端。

首次连接一个未知 SSH 主机时，请先在系统终端使用 `ssh` 核对服务器指纹并写入 `known_hosts`。密钥发生变化时客户端拒绝连接，不自动替换记录。

## Windows 版

客户端代码跨平台（Tauri 2 + WebView2 + ConPTY，需 Windows 10 1809+）。构建方式见 [docs/windows.md](docs/windows.md)：

- **GitHub Actions**：推送到 GitHub 后手动触发 `.github/workflows/windows-build.yml`，产物为 NSIS/MSI 安装包
- **本地构建**：Windows 电脑上 `cd client\ui && npm ci && npx tauri build`

Windows 使用系统原生标题栏（侧栏开关为 Ctrl+B）；安装包未签名，首次运行需通过 SmartScreen 确认。Windows 版 rc-agent 尚未交付，瞬移协议会话暂需 macOS/Linux 被控端。

## 从源码运行

需要 Rust 稳定工具链（本次在 1.98.1 验证）、Node.js 22 或更新版本、macOS Xcode Command Line Tools。Linux 构建独立 Agent / 中继需要 `pkg-config`、OpenSSL 开发包和 C/C++ 工具链；完整桌面客户端还需要 Tauri 的平台依赖。

```bash
cargo build --locked -p rc-server -p rc-agent
npm --prefix client/ui ci
```

在第一台终端启动中继（默认仅监听回环地址）：

```bash
UNIRC_BIND=127.0.0.1:8080 cargo run --locked -p rc-server
```

在被控机器上初始化凭据并启动 Agent：

```bash
cargo run --locked -p rc-agent -- init
cargo run --locked -p rc-agent -- export-certificate ./my-device.shunyi-cert
UNIRC_SERVER=ws://127.0.0.1:8080 cargo run --locked -p rc-agent -- run
```

另一台本地终端生成临时密码或管理凭据：

```bash
cargo run --locked -p rc-agent -- temporary-password 30
cargo run --locked -p rc-agent -- status
cargo run --locked -p rc-agent -- rotate-certificate
cargo run --locked -p rc-agent -- rotation 168
cargo run --locked -p rc-agent -- revoke-temporary
```

`temporary-password` 的参数是未使用密码的有效分钟数（1–1440，默认 30）；密码只在生成命令的标准输出中显示一次。`rotation` 的单位是小时，`off` 关闭自动轮换；默认关闭。自动轮换在 Agent 运行时检查，不会生成并公开新密码。

设备凭据默认在 `~/.shunyi/`，可用 `UNIRC_STATE_DIR` 或 `rc-agent --state-dir PATH` 指定。目录权限为 700，凭据与导出证书为 600。不要把这个目录或证书文件提交到 Git。

启动桌面开发窗口：

```bash
cd client
ui/node_modules/.bin/tauri dev
```

开发预览固定使用 `127.0.0.1:5181`。浏览器预览可以检查界面，真实终端和凭据管理需要原生窗口。

## 自托管与后台运行

UniRC-T v2 需要客户端、Agent 和中继配套更新，不兼容旧版未认证连接流程。

参见 [部署说明](deploy/README.md)。中继只提供设备注册和加密字节转发，不提供账号服务，也不会向匿名客户端公布全平台设备列表。

公网部署请使用 WSS。可在 HTTPS 反向代理后运行回环 HTTP 中继，或同时设置 `UNIRC_TLS_CERT` / `UNIRC_TLS_KEY` 使用内置 TLS。外层证书需要由系统信任；客户端不会跳过证书验证。

设备端与客户端之间始终使用 **TLS 1.3**。设备 ID 绑定设备 CA 证书的 SHA-256 摘要；临时密码在确认设备身份后的 TLS 通道内传输。设备认证成功前不会创建 PTY。完整边界见 [安全说明](SECURITY.md) 和 [协议说明](protocol/UniRC-T-v2.md)。

## 构建与验证

```bash
cargo test --locked -p rc-access -p rc-core -p rc-agent -p rc-server
cargo clippy --locked -p rc-access -p rc-agent -p rc-server --all-targets -- -D warnings
npm --prefix client/ui run build
python3 scripts/build_macos.py
python3 scripts/package_macos.py
python3 scripts/package_source.py --out release/shunyi-0.1.0-source.zip
```

原生构建脚本过滤可能导致 Rust 构建脚本失败的非 UTF-8 环境变量，无需移动中文目录。macOS 分发给其他用户前还需要发布者的 Developer ID 签名与公证；本地开发签名不等于 Apple 公证。

端到端测试启动真实中继、真实 Agent 和真实 PTY，并使用桌面端共用的 `rc-access` 客户端，覆盖证书、临时密码并发、断开重用、证书轮换、设备身份与通道归属。CI 提供 macOS/Linux 的独立 Agent 与中继测试；未运行的 CI 和跨公网测试不作为已验收结论。

## 目录

| 目录 | 内容 |
| --- | --- |
| `crates/rc-core` | 帧编解码及旧协议迁移标识 |
| `crates/rc-access` | 设备凭据、TLS 认证、共享访问客户端 |
| `crates/rc-server` | 无账号中继、注册签名验证、连接归属约束 |
| `crates/rc-agent` | 被控端、PTY 生命周期、凭据 CLI |
| `client` | Tauri 2 + xterm.js 原生客户端 |
| `deploy` | 后台服务与反向代理示例 |
| `protocol` | 当前协议与旧协议历史 |

## 许可与品牌

代码沿用仓库已声明的 Apache-2.0 许可，详见 [LICENSE](LICENSE) 和 [NOTICE](NOTICE)。研发版权主体为北京链脉科技有限公司；瞬移名称、蓝色闪电 S 标志与 UniRC-T 技术标识沿用现有项目。开源许可证不授予商标使用权。

普通问题通过仓库 issue 反馈，贡献流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。安全问题使用 [SECURITY.md](SECURITY.md) 中的私密渠道。
