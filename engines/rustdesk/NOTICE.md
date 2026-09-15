# 瞬移远控引擎预览

本引擎采用 GNU Affero General Public License v3.0（AGPL-3.0-only）；完整许可见同目录 `LICENSE`。引擎不是 RustDesk 官方发行版。

固定上游：<https://github.com/rustdesk/rustdesk/tree/851d2df88cc8ef7a8368f74e8b2e7254861ee00a>。
使用 `libs/scrap` 的平台采集和 VP8 编解码、`libs/enigo` 的输入适配，以及 `libs/base` 的媒体消息类型；`src/macos.mm` 的屏幕缩放函数来自该版本的平台适配。上游及其依赖的版权声明保留在原始源码和锁定依赖中。

UniRC 承担设备证书、临时密码、TLS 认证、授权、桌面会话和画面分片；本阶段的压缩画面仍使用 RustDesk `VideoFrame` 的 Protobuf 表示。未使用 RustDesk 公共中继、设备目录或账号服务。

独立进程用于生命周期和崩溃隔离，不能据此断言免除组合发行物的 AGPL 义务。公开发布组合预览版前，须随发行物提供适用的许可、完整对应源码及构建说明，并检查所有第三方依赖的通知与许可证。仓库既有 0.1 终端代码的 Apache-2.0 声明不代表新增 RustDesk 引擎可按 Apache-2.0 分发。

本地构建：在仓库根目录运行 `python3 scripts/build_desktop_preview.py`；可用 `--debug` 构建调试版。固定上游源码由脚本获取到 `engines/rustdesk/vendor/rustdesk`，不会写入既有终端安装目录。本预览包尚未公证，也未作为新的公开发行版上传。
