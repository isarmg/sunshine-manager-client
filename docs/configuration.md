# Sunshine Client 配置指南

本文适用于 `sunshine-client` `0.2.3`。Client 必须先连接本机 Sunshine，再与 Sunshine Manager 配对；以下命令不会安装 Sunshine。

## 1. 前置条件与状态目录

先在 Sunshine Web UI 中启用管理接口并设置独立的用户名和强密码。本机地址必须是 HTTPS 回环 IP，例如 `https://127.0.0.1:47990/`；不接受 HTTP、主机名、局域网地址、URL 凭据、查询参数或额外路径。

默认状态目录：

| 平台 | 状态目录 |
|---|---|
| Windows | `C:\ProgramData\SunshineClient` |
| Linux | `/var/lib/sunshine-client` |
| macOS | `/Library/Application Support/sunshine-client` |

`--config` 指向整个状态目录，而不是某个 JSON 文件。先确认版本并停止后台服务：

```sh
sunshine-client version --format json
sudo sunshine-client service status --format json
sudo sunshine-client service stop
```

Windows 请在管理员 PowerShell 中去掉 `sudo`；PATH 未刷新时使用：

```powershell
$Client = "$env:ProgramFiles\SunshineClient\sunshine-client.exe"
& $Client version --format json
& $Client service stop
```

## 2. 初始化本机设置

使用默认 Sunshine 地址初始化，或交互输入另一个回环 HTTPS 端口：

```sh
sudo sunshine-client config init
# 或
sudo sunshine-client config init --interactive
sudo sunshine-client config show --format json
```

已有状态时不要再次运行 `config init`。`config show` 返回脱敏配置和 `stored_revision`；当前可编辑设置只有：

```json
{
  "sunshine_endpoint": "https://127.0.0.1:47990/"
}
```

交互编辑：

```sh
sudo env EDITOR="${EDITOR:-vi}" sunshine-client config edit
```

使用候选文件评审和提交：

```sh
sudo install -m 0600 /dev/null /root/sunshine-settings.json
sudoedit /root/sunshine-settings.json
sudo sunshine-client config validate --file /root/sunshine-settings.json --format json
sudo sunshine-client config diff --file /root/sunshine-settings.json --format json
sudo sunshine-client config apply \
  --file /root/sunshine-settings.json \
  --expected-revision COPY_STORED_REVISION_HERE \
  --format json
```

候选文件只允许 `sunshine_endpoint`。提交前重新读取 revision；发生冲突时重新生成候选文件，不要覆盖其他管理员的修改。

## 3. 与 Manager 配对

在 Manager 管理页创建实例并复制授权码。准备严格 JSON：

```json
{
  "server": "https://manager.example.com",
  "authorization_code": "REPLACE_WITH_INSTANCE_AUTHORIZATION_CODE",
  "sunshine_endpoint": "https://127.0.0.1:47990/",
  "sunshine_username": "sunshine-api-user",
  "sunshine_password": "REPLACE_WITH_SUNSHINE_PASSWORD"
}
```

Linux 上创建受保护输入并通过 stdin 配对：

```sh
sudo install -m 0600 /dev/null /root/sunshine-bootstrap.json
sudoedit /root/sunshine-bootstrap.json
sudo sh -c 'exec sunshine-client pair --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo sunshine-client pair status --format json
sudo shred -u /root/sunshine-bootstrap.json
```

有人值守时也可运行完整向导：

```sh
sudo sunshine-client setup --interactive
```

`setup` 还会询问服务启动策略并验证连接；直接 `pair` 只处理配对，更适合分步骤部署。响应丢失时不要清空状态，先执行：

```sh
sudo sunshine-client pair status --format json
sudo sunshine-client pair resume --interactive
```

## 4. 更新 Sunshine 凭据

Sunshine 用户名或密码改变后，停止服务并交互更新：

```sh
sudo sunshine-client service stop
sudo sunshine-client credentials update --interactive
sudo sunshine-client doctor --sunshine --format json
sudo sunshine-client service start
```

自动化输入格式为：

```json
{
  "sunshine_username": "sunshine-api-user",
  "sunshine_password": "REPLACE_WITH_NEW_PASSWORD"
}
```

```sh
sudo install -m 0600 /dev/null /root/sunshine-credentials.json
sudoedit /root/sunshine-credentials.json
sudo sh -c 'exec sunshine-client credentials update --input-stdin --non-interactive --format json < /root/sunshine-credentials.json'
sudo shred -u /root/sunshine-credentials.json
```

此操作保留 Manager 绑定、installation ID 和任务记录。

## 5. 更换 Manager 授权码

Manager 管理员轮换实例授权码后，旧 Client 凭据立即失效。停止服务，创建与首次配对相同格式的新 Bootstrap JSON，再执行：

```sh
sudo sunshine-client service stop
sudo sh -c 'exec sunshine-client pair replace --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo sunshine-client pair status --format json
sudo sunshine-client service start
```

`pair replace` 保留本机安装身份和执行记录。切换到另一台 Manager 前，应先在旧 Manager 退役设备并按运维策略归档状态。

## 6. 启动和验证

```sh
sudo sunshine-client service enable
sudo sunshine-client service start
sudo sunshine-client service status --format json
sunshine-client status --check --format json
sunshine-client doctor --network --format json
sunshine-client doctor --sunshine --format json
sunshine-client tasks list --format json
```

`doctor --network` 检查 Manager 网络入口；`doctor --sunshine` 会真实访问本机 Sunshine API 并验证版本和凭据。最后在 Manager 管理页确认设备在线，再执行一个只读任务验证完整链路。

查看某项本地执行记录：

```sh
sunshine-client tasks show op_REPLACE_WITH_OPERATION_UUID --format json
sunshine-client logs --tail 100
```

## 7. 安全注意事项

- Manager 连接始终校验系统信任链和域名；先修复证书，不要关闭校验。
- 本机 Sunshine 连接被限制为 HTTPS loopback；Client 不校验其自签名证书身份，但每次请求仍校验用户名和密码。
- 授权码和 Sunshine 密码只通过受保护终端或 stdin 输入，不放入参数、环境变量、日志和版本库。
- 配置、配对和凭据写入前停止服务；成功验证后再启动。
