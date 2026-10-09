# xscc 配置指南

首次部署或日常维护请先阅读[分平台全流程指南](platform-setup.md)：按本机平台完成安装、配对、重新配对、服务/后台任务查看与启停、诊断和卸载，命令旁均说明用途。本文详细说明配置字段和业务操作。

本文适用于 `xscc` `1.0.0`。Client 必须先连接本机 Sunshine，再与 xscs 配对；以下命令不会安装 Sunshine。

## 命令用途与执行边界

| 命令 | 用途与影响 |
|---|---|
| `version --format json` | 只读确认软件、平台及当前协议/账户版本 |
| `config init [--interactive]` | 首次初始化本机 Sunshine 接口设置，不用于清理旧身份 |
| `config show --format json` | 查看脱敏设置和 `stored_revision` |
| `config edit` | 在编辑器中修改候选 Sunshine endpoint，经校验和修订检查后提交；先停服 |
| `config validate --file PATH` | 仅校验候选，不应用它 |
| `config diff --file PATH` | 比较候选与当前设置，供审阅 |
| `config apply --file PATH --expected-revision REVISION` | 检查当前修订并提交候选，避免并发覆盖；先停服 |
| `setup` | 完整配对/复用、服务策略、启动和连接验证，适合首次部署 |
| `pair` / `pair status` / `pair resume` | 分别执行单独配对、查看事务、恢复已开始事务 |
| `pair replace` | 用新 Manager 授权替换绑定，保留本机安装身份和执行记录 |
| `credentials update` | 更新本机 Sunshine 用户名/密码，保留 Manager 绑定；先停服 |
| `status --check` | 查看 Client 业务与连接检查，不能代替只读任务往返验收 |
| `doctor --network` | 检查远程 Manager 入口，不能证明 Sunshine 本机认证可用 |
| `doctor --sunshine` | 真实访问本机 Sunshine API，检查版本与凭据 |
| `tasks list` / `tasks show OPERATION_ID` | 查看任务列表/单项持久执行记录，不重新执行任务 |
| `logs --tail 100` | 读取最近日志，不改变服务运行状态 |

`--config` 指向整个状态目录，`--file` 指向实际候选文件，`REVISION` 从最新 `config show` 复制。`--interactive` 通过受保护终端输入，`--input-stdin` 从受保护 stdin 读严格 JSON；`--non-interactive` 禁止额外提示，`--timeout` 限定等待，`--format json` 提供结构化结果。系统服务操作必须使用默认状态目录。


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
sudo xscc config init
# 或
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
sudo sh -c 'exec xscc pair --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo xscc pair status --format json
sudo shred -u /root/sunshine-bootstrap.json
```

有人值守时也可运行完整向导：

```sh
sudo xscc setup --interactive
```

首次配对和 `pair replace --interactive` 的 Manager 实例授权码都使用 `Authorization code (visible)` 普通
文本提示，输入或粘贴内容会在终端中明文回显，不提供遮罩、隐藏切换或特殊显示流程。本机 Sunshine 密码
仍使用隐藏输入。CLI 不会把两者写入日志、结果 JSON 或命令参数。

`setup` 仅询问是否开机自启，默认 Yes，直接按 Enter 即可；完成配对后固定启动后台服务并验证连接。
当前平台支持的管理功能全部启用，没有重启、应用、配对、诊断或维护权限开关。
`setup --input-stdin --non-interactive` 默认开机自启、立即启动并验证，不再读取额外确认输入。
直接 `pair` 只处理配对，适合分步骤部署。响应丢失时不要清空状态，先执行：

```sh
sudo xscc pair status --format json
sudo xscc pair resume --interactive
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
sudo shred -u /root/sunshine-credentials.json
```

此操作保留 Manager 绑定、installation ID 和任务记录。

## 5. 更换 Manager 授权码

Manager 管理员轮换实例授权码后，旧 Client 凭据立即失效。停止服务，创建与首次配对相同格式的新 Bootstrap JSON，再执行：

```sh
sudo xscc service stop
sudo sh -c 'exec xscc pair replace --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo xscc pair status --format json
sudo xscc service start
```

`pair replace` 保留本机安装身份和执行记录。切换到另一台 Manager 前，应先在旧 Manager 退役设备并按运维策略归档状态。

若旧版本账户文档返回 `pairing_state_incompatible`，使用同一条 `pair replace` 命令。Client 会在确认执行日志完整后归档旧账户文档并创建当前身份；如果执行日志不兼容或不可读，则返回 `important_state_incompatible`，不会删除、改写或绕过这些重要记录。

## 6. 启动和验证

完整 `setup` 已自动启动并验证服务。以下命令用于后续查看或显式恢复已停用的服务：

```sh
sudo xscc service enable
sudo xscc service start
sudo xscc service status --format json
xscc status --check --format json
xscc doctor --network --format json
xscc doctor --sunshine --format json
xscc tasks list --format json
```

`doctor --network` 检查 Manager 网络入口；`doctor --sunshine` 会真实访问本机 Sunshine API 并验证版本和凭据。最后在 Manager 管理页确认设备在线，再执行一个只读任务验证完整链路。

两个网络探测均遵守 `--timeout`（如 `--timeout 5s`）；Manager 探测超时返回 `network_probe_timeout`，本机 Sunshine 探测超时返回 `sunshine_probe_timeout`。已到达本机 Sunshine API 的探测在成功、凭据拒绝和版本不受支持时均标明 TLS 已加密、证书身份未按回环策略检查。

查看某项本地执行记录：

```sh
xscc tasks show op_REPLACE_WITH_OPERATION_UUID --format json
xscc logs --tail 100
```

## 7. 安全注意事项

- Manager 连接始终校验系统信任链和域名；先修复证书，不要关闭校验。
- 本机 Sunshine 连接被限制为 HTTPS loopback；Client 不校验其自签名证书身份，但每次请求仍校验用户名和密码。
- 授权码通过受保护终端的明文回显提示或 stdin 输入；Sunshine 密码通过隐藏提示或 stdin 输入。两者都不放入参数、环境变量、日志和版本库。
- 配置、配对和凭据写入前停止服务；成功验证后再启动。

## 运行日志

Manager 连接、断开、授权拒绝和协议失败使用 Foundation 同一 `xcss-log` 实现输出 UTC JSON 行到 stderr，事件为 `xscc.session.*`。每条带设备 UUID 的 `instance_id` 和稳定失败 `error_code`；不输出凭据、Manager endpoint 或任意内部错误链。诊断写入失败返回 `diagnostics_unavailable` 并停止运行，由平台服务宿主报告。
