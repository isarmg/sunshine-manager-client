# 配置 xscc

首次部署直接使用[安装指南](platform-setup.md)的 `setup --interactive`；下面用于按步骤配置、审阅修改或自动化。修改配置、配对和凭据前先停止服务，验证后恢复运行。

命令总表见[命令参考](cli-compatibility.md)。所有 `REPLACE_...`、`COPY_STORED_REVISION_HERE` 和 `CURRENT_HOST_UUID` 都要替换为实际值。秘密输入放在受保护文件或交互提示中。

## 1. 前置条件与状态目录

先在 Sunshine Web UI 中启用管理接口并设置独立的用户名和强密码。本机地址必须是 HTTPS 回环 IP，例如 `https://127.0.0.1:47990/`；不接受 HTTP、主机名、局域网地址、URL 凭据、查询参数或额外路径。

默认状态目录：

| 平台 | 状态目录 |
|---|---|
| Windows | `C:\ProgramData\Xscc` |
| Linux | `/var/lib/xscc` |
| macOS | `/Library/Application Support/xscc` |

`--config` 指向整个状态目录，而不是某个 JSON 文件。先确认版本并停止后台服务：

```sh
xscc version --format json
sudo xscc service status --format json
sudo xscc service stop
```

Windows 请在管理员 PowerShell 中去掉 `sudo`；PATH 未刷新时使用：

```powershell
$InstallRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\xscc').InstallLocation
$Client = Join-Path $InstallRoot 'xscc.exe'
& $Client version --format json
& $Client service stop
```

## 2. 初始化本机设置

使用默认 Sunshine 地址初始化，或交互输入另一个回环 HTTPS 端口：

```sh
sudo xscc config init --interactive
sudo xscc config show --format json
```

已有状态时不要再次运行 `config init`。`config show` 返回脱敏配置和 `stored_revision`；当前可编辑设置只有：

```json
{
  "sunshine_endpoint": "https://127.0.0.1:47990/"
}
```

交互编辑：

```sh
sudo env EDITOR="${EDITOR:-vi}" xscc config edit
```

使用候选文件评审和提交：

```sh
sudo install -m 0600 /dev/null /root/sunshine-settings.json
sudoedit /root/sunshine-settings.json
sudo xscc config validate --file /root/sunshine-settings.json --format json
sudo xscc config diff --file /root/sunshine-settings.json --format json
sudo xscc config apply \
  --file /root/sunshine-settings.json \
  --expected-revision COPY_STORED_REVISION_HERE \
  --format json
```

候选文件只允许 `sunshine_endpoint`。提交前重新读取提交修订；发生冲突时重新生成候选文件，不要覆盖其他管理员的修改。

## 3. 与管理端配对

在管理端管理页创建实例并复制授权码。准备严格 JSON：

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
sudo sh -c 'exec xscc pair --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo xscc pair status --format json
sudo rm -f /root/sunshine-bootstrap.json
```

有人值守时也可运行完整向导：

```sh
sudo xscc setup --interactive
```

实例授权码在交互终端明文回显，Sunshine 密码隐藏输入；自动化使用受保护 stdin。

`setup` 仅询问是否开机自启，默认 Yes，直接按 Enter 即可；完成配对后固定启动后台服务并验证连接。
当前平台支持的管理功能全部启用，没有重启、应用、配对、诊断或维护权限开关。
`setup --input-stdin --non-interactive` 默认开机自启、立即启动并验证，不再读取额外确认输入。
直接 `pair` 只处理配对，适合分步骤部署。响应丢失时不要清空状态，先执行：

```sh
sudo xscc pair status --format json
sudo xscc pair resume
```

## 4. 更新 Sunshine 凭据

Sunshine 用户名或密码改变后，停止服务并交互更新：

```sh
sudo xscc service stop
sudo xscc credentials update --interactive
sudo xscc doctor --sunshine --format json
sudo xscc service start
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
sudo sh -c 'exec xscc credentials update --input-stdin --non-interactive --format json < /root/sunshine-credentials.json'
sudo rm -f /root/sunshine-credentials.json
```

此操作保留管理端绑定、installation ID 和任务记录。

## 5. 更换管理端授权码

管理端管理员轮换实例授权码后，旧客户端凭据立即失效。停止服务，创建与首次配对相同格式的新引导配置 JSON，再执行：

```sh
sudo xscc service stop
sudo sh -c 'exec xscc pair replace --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo xscc pair status --format json
sudo xscc service start
```

`pair replace` 保留本机安装身份和执行记录。切换到另一台管理端前，应先在旧管理端退役设备并按运维策略归档状态。

## 6. 启动和验证

完整 `setup` 已自动启动并验证服务。以下命令用于后续查看或显式恢复已停用的服务：

```sh
sudo xscc service enable
sudo xscc service start
sudo xscc service status --format json
sudo xscc status --check --format json
sudo xscc doctor --network --format json
sudo xscc doctor --sunshine --format json
sudo xscc tasks list --format json
```

`doctor --network` 检查管理端网络入口；`doctor --sunshine` 会真实访问本机 Sunshine API 并验证版本和凭据。最后在管理端管理页确认设备在线，再执行一个只读任务验证完整链路。

两个网络探测均遵守 `--timeout`（如 `--timeout 5s`）；管理端探测超时返回 `network_probe_timeout`，本机 Sunshine 探测超时返回 `sunshine_probe_timeout`。已到达本机 Sunshine API 的探测在成功、凭据拒绝和版本不受支持时均标明 TLS 已加密、证书身份未按回环策略检查。

查看某项本地执行记录：

```sh
sudo xscc tasks show op_REPLACE_WITH_OPERATION_UUID --format json
sudo xscc logs --tail 100
```
