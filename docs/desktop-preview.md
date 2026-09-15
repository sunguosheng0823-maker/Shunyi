# 瞬移远控预览

这一阶段在终端客户端中加入桌面标签页：项目和全局远程主机仍分别管理，终端保持原有布局。远控采用 RustDesk 引擎与 UniRC 认证/传输相结合的实现，账号业务仍不启用。

## 使用

1. 在被控设备安装包含远控引擎的预览包。打开「设置 → 本机接入与凭据」，选择「仅允许查看屏幕」或「允许查看和控制键盘鼠标」，再开始共享。
2. macOS 需要录屏权限；发送键鼠还需要辅助功能权限。窗口中的权限按钮打开对应系统设置。系统没有授权时，程序会拒绝采集或控制，不会自动提升为 root。
3. 中继也需更新为支持 UniRC v3 的版本（兼容原有 v2 终端）；旧 0.1 中继无法建立桌面连接。控制端在设置中填入自建中继地址，添加瞬移协议主机，导入对方的证书或输入临时密码。
4. 主机行右侧的屏幕按钮打开只读桌面；右键主机，选择「控制远程桌面」建立控制会话。关闭查看会话后才能更换控制模式。
5. 点击画面后使用键鼠；`Ctrl + Alt + Escape` 退出画面焦点。切换标签页、离开画面或断开时会请求释放按键。预览版尚未同步远端光标图像。

开启桌面共享后，持有效证书或本次临时密码的使用者可以获得所选权限。更改策略前停止共享。临时密码在设备认证成功时已被使用；录屏权限不足导致桌面失败时，也需重新生成临时密码。使用长期证书可在修好系统权限后直接重连。

独立预览包使用 `.shunyi-desktop-preview` 设备状态目录和独立应用标识；正式 0.1 的 `.shunyi` 设备凭据和已安装应用保持原有用途。独立 Agent 通过 `UNIRC_DESKTOP_PERMISSION=view|control` 与 `SHUNYI_DESKTOP_ENGINE=/absolute/path/to/engine` 显式开启桌面；默认仍只提供终端。

## 构建

当前打包脚本仅支持 macOS，需要 Rust、Node、Xcode Command Line Tools、Homebrew 和 CMake。

```bash
brew install libvpx aom pkgconf cmake
npm --prefix client/ui ci
python3 scripts/build_desktop_preview.py
```

输出包含 `target/desktop-preview/瞬移远控预览.app`、同目录的 `rc-agent`、`rc-server` 和 `build.json`。脚本不会覆盖 `/Applications/瞬移.app`，不会开启共享，也不会授予系统权限。`--debug` 可生成调试版。

RustDesk 与 libyuv 固定到具体提交，Rust 依赖由独立 `Cargo.lock` 锁定。Homebrew 原生依赖的实际版本记录在构建回执中，尚未提供所有依赖的完全可复现制品。构建机的旧版 macOS 兼容性、Intel Mac、Windows 构建和跨机互控都需要进一步验收。

源码边界、AGPL 声明和上游来源见 `engines/rustdesk/NOTICE.md`；协议见 `protocol/UniRC-T-v3-desktop-preview.md`。这是本地预览包，尚未公证，也没有作为新版本发布到公开市场。公开发布前须补齐组合发行物的对应源码、依赖通知与平台验收。

## 已有范围与后续范围

当前实现设备认证后的单屏画面、VP8 软件编解码、独立桌面标签页、只读/控制策略、基础键鼠、断开清理和与终端共存。运行中实际画面与真实键鼠的验证结果必须分别查看验收报告。

后续仍需跨 Windows/Mac 验收、弱网与长时间稳定性、分辨率变化/显示器热插拔、多屏切换、光标同步、音频、剪贴板、文件传输、无人值守服务和安装更新方案。macOS 锁屏/登录前、Windows 提权窗口和安全桌面不在本预览的交付范围内。

## 验收与复现

本次执行记录见 [验收报告](verification/desktop-preview-2026-09-15/README.md)。`cargo test --workspace --all-targets` 覆盖原有终端与新增桌面协议；`shunyi-desktop-engine self-test` 验证真实 VP8 编解码，不采集屏幕或发送输入。

原生验收使用 `rc-server/examples/native_fixture.rs` 创建仅监听回环地址的临时环境，再运行 `scripts/verify_desktop_preview.py <fixture-directory> --engine <absolute-engine-path>` 检查实际认证、采集与解码。先构建 `cargo build -p rc-server --example desktop_acceptance --example native_fixture`。测试目录包含私有凭据，应留在本机，不能放入公开验收附件。

追加 `--input` 会把独立测试窗口切到前台并发送少量键鼠事件，执行前须明确取得本机使用者同意。脚本检查前台进程，验证点击、按键，以及断开后释放未抬起的按键，结束时关闭自己的测试窗口。系统未授予辅助功能权限时，结果应记录为未通过，不能用编解码自测代替实际控制验收。
