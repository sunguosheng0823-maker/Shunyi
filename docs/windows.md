# Windows 桌面版构建指南

客户端是 Tauri 2 + Rust，代码本身跨平台（Windows 走 WebView2 + ConPTY）。
**Windows 包无法在 macOS 上交叉编译**，需要以下两种方式之一。

## 方式一：GitHub Actions 自动构建（推荐）

1. 把仓库推到 GitHub（私有仓库即可）
2. 仓库页 → Actions → 「Windows 构建（瞬移客户端）」→ Run workflow
3. 跑完后在该次运行页面下载 artifact `shunyi-windows-<sha>`，内含：
   - `瞬移_0.1.0_x64-setup.exe`（NSIS 安装器）
   - `瞬移_0.1.0_x64_en-US.msi`（MSI 安装包）

工作流文件：`.github/workflows/windows-build.yml`（手动触发，不占用自动 CI）。

## 方式二：在 Windows 电脑/虚拟机上本地构建

### 前置要求

| 组件 | 说明 |
|---|---|
| Windows 10 1809+ | 终端依赖 ConPTY（1809 起内置） |
| Rust (MSVC) | `rustup` 安装 stable，需 Visual Studio 2022 Build Tools（含 C++ 工具集） |
| Node.js | 18+（建议 20 LTS） |
| WebView2 | Win10 21H2+/Win11 系统自带；老系统装 Evergreen Runtime |

### 构建步骤

```powershell
cd client\ui
npm ci
npx tauri build
```

产物在 `client\src-tauri\target\release\bundle\` 下：

- `nsis\瞬移_0.1.0_x64-setup.exe`
- `msi\瞬移_0.1.0_x64_en-US.msi`

开发调试（热重载）：

```powershell
cd client\ui
npx tauri dev
```

## 安装与分发注意

1. **SmartScreen 告警**：安装包未签名（与 mac 版同理），首次运行点「更多信息 → 仍要运行」。
   正式分发前建议购买代码签名证书（OV 起步即可消除告警，EV 可直接免告警）。
2. **数据目录**：设置/主机/指令存在 `%APPDATA%\cn.wkkj.shunyi\`（EBWebView 存储随 identifier 走）。
3. **默认终端**：Windows 下默认 PowerShell（读 `COMSPEC`）；起始目录、快捷指令、项目制等功能与 mac 版一致。
4. **标题栏**：Windows 用系统原生标题栏（最小化/关闭按钮），侧栏开关快捷键为 **Ctrl+B**（mac 为 ⌘B）；⌘P/⌘O 对应 Ctrl+P/Ctrl+O。

## 已知限制（截至 0.1.0）

- Windows 版尚未在真机做过系统化测试，首次装机建议按冒烟清单过一遍：本地终端、新建 SSH 主机连接、文件树预览、快捷指令发送、导入导出。
- SSH 主机密钥校验目前直接接受（known_hosts/TOFU 在 M1 路线）。
- 瞬移协议（自有协议）会话需要部署 rc-server + 被控机安装 rc-agent；**Windows 版 rc-agent 尚未构建**（单文件 Agent 的 Windows 服务化在 M1-M2「Windows 先行」阶段交付）。
- 未开启端到端加密（M1），当前中继流量为明文 WS——生产部署前先上 WSS/TLS。

## 排障

- **构建报 env 编码/UTF-8 panic**：项目路径含中文时若复现 macOS 同款 libm panic（Windows 理论上不受影响，env 为 UTF-16 存储），把仓库移到纯 ASCII 路径（如 `C:\build\shunyi`）再打包。
- **`error: linker 'link.exe' not found`**：VS Build Tools 未装或未装 C++ 工具集，重装勾选「使用 C++ 的桌面开发」。
- **WebView2 白屏**：老系统安装 [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
