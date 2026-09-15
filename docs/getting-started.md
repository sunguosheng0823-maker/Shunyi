# 快速开始

免账号远程连接包含三个角色：主控 Mac 客户端、被控设备上的 Agent、双方都能访问的中继。中继可与 Agent 部署在同一台机器，但它们是不同进程。SSH 直连不需要 Agent 或中继。

## 选择连接方式

| 需求 | 使用方式 |
| --- | --- |
| 本机终端与项目文件 | 点击“打开项目”，选择目录 |
| 已有 SSH 服务器 | 新建主机选择 SSH，填写系统用户名及凭据 |
| 长期管理安装了 Agent 的设备 | 使用设备端导出的连接证书 |
| 临时协助他人操作终端 | 使用设备 ID 与一次性临时密码 |

## 在一台 Mac 上验证完整连接

先按 [README](../README.md#从源码运行) 安装依赖并构建后端。本例只使用本机回环网络。

**终端一：运行中继。**

```bash
UNIRC_BIND=127.0.0.1:8080 ./target/debug/rc-server
```

**终端二：初始化设备并运行 Agent。** 本例使用单独的验证目录，避免更换日常设备身份。

```bash
mkdir -p "$HOME/.shunyi-demo"
chmod 700 "$HOME/.shunyi-demo"
./target/debug/rc-agent --state-dir "$HOME/.shunyi-demo" init
./target/debug/rc-agent --state-dir "$HOME/.shunyi-demo" export-certificate "$HOME/.shunyi-demo/demo.shunyi-cert"
UNIRC_SERVER=ws://127.0.0.1:8080 ./target/debug/rc-agent --state-dir "$HOME/.shunyi-demo" run
```

导出不会覆盖已有证书文件。再次验证时直接使用已有证书；轮换过证书后应导出到新文件名。

**Mac 客户端：连接设备。**

1. 在“设置”中填写 `ws://127.0.0.1:8080`，本例不设置中继准入令牌。
2. 新建远程主机，输入名称，选择“瞬移协议”。
3. 连接方式选择“连接证书”，通过文件选择器打开 `~/.shunyi-demo/demo.shunyi-cert`，设备 ID 自动填入。
4. 点击“保存并连接”，执行 `whoami` 和 `pwd`，确认返回的是 Agent 所在机器的用户和目录。

需要使用安装的 App 或 `tauri dev` 原生窗口。浏览器页面只能预览界面，不能创建真实终端。

## 验证一次性临时密码

保持中继与 Agent 运行，在另一个终端执行：

```bash
./target/debug/rc-agent --state-dir "$HOME/.shunyi-demo" status
./target/debug/rc-agent --state-dir "$HOME/.shunyi-demo" temporary-password 30
```

`status` 显示设备 ID，第二条命令显示新密码。客户端新建另一条主机记录，选择“临时密码”，填写设备 ID 与密码并连接。

关闭该次访问的全部终端后，再用相同密码连接应被拒绝。输入错误密码不会消耗正确密码；正确密码已被设备确认后，即使客户端未收到确认，也按已使用处理。临时密码不会保存在主机配置中。

本例密码需要在 30 分钟内首次使用。有效期限制新连接认证，不会到点关闭已经授权的终端。需要更换时重新生成，旧的未使用密码立即失效。

验证结束，在 Agent 和中继终端分别按 Ctrl+C。已使用密码不会因重启恢复有效。

## 连接另一台机器

将中继部署为双方可访问的可信 WSS 地址，例如 `wss://relay.example.com`。`127.0.0.1` 始终指使用它的本机，不能作为两台机器互联时的公共中继地址。

被控 Mac 可以在“设置 → 本机接入与凭据”中开始共享；没有图形界面或需要长期后台运行的设备使用独立 Agent。只向获准操作设备的人分发证书或临时密码。连接证书需归当前用户所有、权限为 600，无需把私钥内容粘贴到界面中。

WSS、后台启动和停止共享的完整步骤见 [部署说明](../deploy/README.md)。

## 凭据、进程与权限

- 默认身份目录为 `~/.shunyi/`，可用 `--state-dir` 或 `UNIRC_STATE_DIR` 指定其他目录。
- 管理凭据必须使用 Agent 的同一系统用户、同一状态目录，否则管理的是另一套设备身份。
- 图形共享与独立 Agent 不能同时占用同一身份目录，进程锁会拒绝重复启动。
- “共享服务运行中”表示进程运行状态。实际可连接性以远端成功打开终端并执行命令为准。
- 远程终端继承 Agent 的系统用户身份；root 权限由设备管理员在设备端明确配置。

## SSH 首次连接

先使用系统终端的 `ssh user@host`，通过可信渠道核对服务器指纹后写入系统 `known_hosts`。再在瞬移中连接同一地址、端口和用户。密钥变化时先确认原因，客户端不会自动信任替换后的密钥。
