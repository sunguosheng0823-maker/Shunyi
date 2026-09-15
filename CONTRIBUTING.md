# 参与贡献

欢迎提交可复现的问题、文档修正及围绕终端工作区的改进。当前范围为免账号设备访问；账号系统和远程桌面请先在 issue 中讨论接口与范围。

## 提交问题

说明系统版本、CPU 架构、瞬移版本、连接方式、最短复现步骤、预期与实际结果。附上脱敏的错误文本。

移除证书私钥、临时密码、中继令牌、生产 IP 和个人目录信息。安全漏洞不要通过公开 issue 报告，见 [SECURITY.md](SECURITY.md)。

## 开发与验证

依赖与启动步骤见 [README](README.md#从源码运行)。浏览器可预览界面，真实终端、文件访问及凭据功能需要在 Tauri 原生窗口验证。请为开发 Agent 指定单独的测试身份目录。

一次变更围绕一个具体问题，说明触发条件、行为变化与验证方式。协议和凭据规则变化时，同步更新安全说明及相关测试。

后端变更运行：

```bash
cargo test --locked -p rc-access -p rc-core -p rc-agent -p rc-server
cargo clippy --locked -p rc-access -p rc-agent -p rc-server --all-targets -- -D warnings
```

客户端变更运行：

```bash
npm --prefix client/ui ci
npm --prefix client/ui run build
cargo test --locked -p unirc-client --lib
```

端到端测试使用临时身份目录、回环中继和 PTY，不需要生产凭据。界面变更检查 940×600 最小窗口、键盘焦点及错误恢复；凭据变更检查认证成功、失败与失效。明确说明未验证的平台或部署环境。

新增 Rust 文件使用 rustfmt，避免重排无关代码。测试应验证用户行为、权限边界或实际故障，避免只重复实现。

## 发布内容

保留 `Cargo.lock` 和前端锁文件。不得提交身份目录、连接证书、生产配置、真实环境变量文件、依赖目录、构建产物和个人验收记录。

源码分发使用 `scripts/package_source.py` 中的明确清单，新增公开文档时同步清单。`scripts/package_macos.py` 收集依赖许可和必要源码归档，并检查本地开发签名；Apple Developer ID 签名及公证由维护者另行完成。

贡献者需有权以本项目 Apache-2.0 许可提供相应内容。保留第三方来源及许可说明，不提交授权来源不明的素材或代码。
