# 独立 Agent 与中继部署

下面是供维护者按目标机器配置的示例，不会在开发机上自动安装或启用服务。桌面客户端不必常驻；无人值守机器只需要一个 Agent 后台服务。

## Linux：开机启动

先构建 `cargo build --locked --release -p rc-agent -p rc-server`，将对应二进制安装到 `/usr/local/bin/`。

`systemd/shunyi-agent.service` 为设备端模板。把 `User=shunyi` 改成允许执行终端命令的真实系统用户，设置 `agent.env.example` 中的中继和状态目录，保存为 `/etc/shunyi/agent.env`（600）。预先创建凭据目录，例如 `/var/lib/shunyi-agent`，设为该用户所有、权限 700。

只有明确需要 root 终端且被控机器管理员授权时，才将 `User=` 设置为 `root`。证书与临时密码的持有者将获得此服务用户的 Shell 权限。

确认配置后安装并启用：

```bash
sudo cp deploy/systemd/shunyi-agent.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now shunyi-agent
sudo systemctl status shunyi-agent
```

必须以相同的服务用户和状态目录运行凭据管理命令。默认模板示例：

```bash
sudo -u shunyi /usr/local/bin/rc-agent --state-dir /var/lib/shunyi-agent status
sudo -u shunyi /usr/local/bin/rc-agent --state-dir /var/lib/shunyi-agent temporary-password 30
sudo -u shunyi /usr/local/bin/rc-agent --state-dir /var/lib/shunyi-agent export-certificate /var/lib/shunyi-agent/server.shunyi-cert
```

停止共享用 `sudo systemctl stop shunyi-agent`；关闭开机启动用 `sudo systemctl disable --now shunyi-agent`。停止服务会结束所有设备访问与其终端，已使用临时密码不会恢复。

中继使用 `systemd/shunyi-relay.service`，需另行创建无特权用户 `shunyi-relay`、配置 `/etc/shunyi/relay.env`，并配可信 HTTPS 代理或内置 WSS。中继不需要 root Shell 权限。

## macOS：登录后自动运行

`launchd/cn.wkkj.shunyi.agent.plist.example` 是用户级 LaunchAgent 模板。将二进制路径、`REPLACE_USER` 和中继地址改成目标机器的实际值，保存到 `~/Library/LaunchAgents/cn.wkkj.shunyi.agent.plist`，权限 600。所有路径必须是绝对路径，plist 不展开 `~` 或 shell 环境变量。

```bash
plutil -lint ~/Library/LaunchAgents/cn.wkkj.shunyi.agent.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/cn.wkkj.shunyi.agent.plist
launchctl print gui/$(id -u)/cn.wkkj.shunyi.agent
```

这会在当前用户登录后自动启动，并在异常退出后重启；不是开机未登录的系统级 LaunchDaemon。Agent 的状态目录与 Mac 客户端共用时，只启用其中一种共享方式，进程锁会阻止重复 Agent。

停用：

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/cn.wkkj.shunyi.agent.plist
```

macOS 用户级服务以该用户运行，不提供自动 root 提权。Windows 后台服务安装与系统级 macOS LaunchDaemon 不在本次交付范围内。

## WSS 与部署验证

`nginx.conf.example` 供已有 HTTPS 站点使用，不包含实际域名、证书或生产凭据。外层 WSS 必须使用系统信任的服务器证书，不能通过关闭验证来连接自签名中继。内层设备 TLS 使用设备 ID 校验的设备 CA，两者相互独立。

部署验收顺序：中继 `/health` 正常 → Agent 通过签名注册 → 使用证书或临时密码打开真实终端 → 执行命令并看到结果 → 断开后旧临时密码被拒绝。仅有健康接口返回成功不等于远程连接验收。

服务模板已提供并进行语法检查，跨平台安装、真实公网与系统重启行为需在实际部署机器上验收。
