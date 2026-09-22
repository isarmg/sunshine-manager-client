# 各平台安装与覆盖升级

适用于 0.2.7。Client 是管理代理；需要先安装 Sunshine 并在其 Web UI 设置用户名、密码。Manager 地址、Manager 配对码、Sunshine 本机地址和 Sunshine 管理凭据是不同的输入。每个平台只发布一个原生安装包，安装器检查平台、架构、权限并注册服务。安装后的逐步配置和配对命令见[完整配置指南](configuration.md)。

## Windows 11 x64

下载并校验 Release 的 Windows MSI。在管理员 PowerShell 中进行普通安装；安装事务只部署程序和服务，不执行交互式 `setup`：

```powershell
msiexec.exe /i .\sunshine-client-0.2.7-windows-x64.msi /norestart
$installRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\Sunshine Client').InstallLocation
$client = Join-Path $installRoot 'sunshine-client.exe'
```

MSI 只安装程序并登记 Manual/Stopped 服务，不启动配对，也不读取任何秘密。默认选中的“Prepare incompatible account data for Setup (recommended)”会先只读验证执行日志，再把不兼容或损坏的 `identity.json` / `bootstrap.json` 归档为唯一名称（包括可识别的 v1 文档）；当前 v2 账户保持不变，执行日志永不由该选项删除。安装完成后，在管理员终端显式运行 `& $client setup --interactive` 即可创建当前账户；原有 CLI 会在需要写入受保护状态时请求提权，`--help` 和 `--version` 不触发 UAC。配对失败不会回滚已提交的安装。

交互输入 Manager 管理台生成的配对码、`https://127.0.0.1:47990/`、Sunshine Web UI 用户名和密码。Manager 配对码按普通文本在终端中明文回显，不提供遮罩或隐藏切换；Sunshine 密码仍隐藏输入。默认服务账户 LocalSystem，状态目录 `C:\ProgramData\SunshineClient`。MSI 会把用户选择的程序目录事务性追加到机器 PATH；新终端可直接运行 `sunshine-client`，卸载会移除该安装器拥有的 PATH 项。

已有版本直接再次运行同一 MSI；原生安装器处理升级、修复、降级检查和服务登记，保留设备身份、凭据、任务记录及启动意图。安装阶段归档不兼容账户后，只需运行普通 `sunshine-client setup` 并输入新的实例授权码；有效 v2 身份仍会直接复用，未完成事务仍会恢复。

交互安装会进入“自定义安装”页：可以修改安装目录，并分别选择是否保留旧的 Manager/Sunshine 配置与配对凭据、是否保留旧的执行日志、是否为 Setup 准备不兼容账户；三项默认选中。取消前两项会在结构安全检查通过后永久清理对应类别；取消兼容性准备则完整保留旧账户，由管理员之后显式处理。向导最终明确显示完成或失败，不会无提示退出。

MSI 同版文件修复：`msiexec.exe /i "完整路径\sunshine-client-0.2.7-windows-x64.msi" REINSTALL=ALL REINSTALLMODE=amus /l*v "%TEMP%\sunshine-client-install.log"`（在 cmd 中执行）。安装器忙碌时等待其他安装结束；3010 表示 Windows 需要重启完成替换。使用 `Get-Service SunshineClient` 和 `sunshine-client service status` 检查服务。

## Ubuntu 24.04 x86_64

```sh
sudo apt install ./sunshine-client_0.2.7_amd64.deb
sudo sunshine-client setup
```

默认配置与状态位于 `/var/lib/sunshine-client`，运行账户 `sunshine-client`。安装后运行 `sudo sunshine-client setup`，按提示选择开机启动、立即启动并验证连接。日志：`sudo journalctl -u sunshine-client.service -n 100 --no-pager`。

同版损坏执行 `sudo apt install --reinstall ./sunshine-client_0.2.7_amd64.deb`。若状态来自 v1，先创建新授权码，再运行 `sudo sunshine-client pair replace --interactive`；Client 会归档旧账户文档，执行日志异常时则保留所有原件并明确报错。若安装器没有交互终端，稍后手动运行该命令。

手工归档只包含可执行文件、文档和校验元数据，不包含 Shell/Python 安装包装脚本；生产安装和覆盖请使用 DEB。解压的可执行文件可用于临时诊断，但不替代原生服务安装器。

## macOS Apple Silicon

只提供 Apple Silicon 原生 PKG，不提供 Intel 版本：

```sh
sudo installer -pkg ./sunshine-client-0.2.7-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/sunshine-client setup
```

状态路径 `/Library/Application Support/sunshine-client`，系统服务账户 `_sunshineclient`；LaunchDaemon `org.sarmg.sunshine-client` 无需用户登录即可运行。诊断命令 `sudo launchctl print system/org.sarmg.sunshine-client`，日志 `/var/log/sunshine-client.log`。

再次运行原生 PKG 即可覆盖旧程序，或在保留状态的卸载后重装。安装器保留身份和执行记录；随后运行 `/usr/local/bin/sunshine-client setup`。发行文件未签名、未公证，使用系统提供的本地批准入口允许已校验的程序运行，不要全局关闭系统安全检查。

## 本机 Sunshine HTTPS 与认证边界

Client 只接受 `https://127.0.0.1:<port>/` 或等价的 IPv6 回环 IP 字面量。主机名、非回环地址、HTTP、URL 用户信息、查询、片段和额外路径都会在连接前被拒绝；请求不使用系统代理，也不跟随重定向。

Sunshine 默认使用本机自签名证书。由于连接被限制在内核回环接口，本版本不校验 Sunshine 证书链、名称、指纹或固定值，也不读取和保存 `cacert.pem`。TLS 仍用于加密连接，每个 API 请求均携带受保护状态中的 Sunshine Basic Auth 凭据；401/403 会作为凭据拒绝处理。Manager 是独立的远程安全边界，始终使用标准 WebPKI 身份校验和设备长期凭据认证。

公开的首次配对入口使用受保护的 stdin JSON：

```json
{"server":"https://manager.example.org/","authorization_code":"REPLACE_WITH_INSTANCE_CODE","sunshine_endpoint":"https://127.0.0.1:47990/","sunshine_username":"REPLACE_LOCALLY","sunshine_password":"REPLACE_LOCALLY"}
```

此 JSON 只可交给 `sunshine-client setup --input-stdin --non-interactive` 的 stdin。当前格式拒绝 `sunshine_certificate`、`sunshine_certificate_path`、`sunshine_ca_pem` 和 `use_system_trust` 等旧字段。秘密不得放进命令参数、部署日志或版本库。`credentials update` 只接受新的 Sunshine 用户名和密码。

## 已有配置、升级与故障处理

`setup` 按配置、配对、服务注册、启动策略、运行状态和连接顺序执行后置验证。交互终端会逐步显示 `verified`；JSON 失败响应中的 `error.step` 指明失败关卡，`error.code` 和 `error.message` 给出稳定原因，操作系统服务命令失败时 `error.detail` 保留经过控制字符清理和长度限制的原始诊断。请求连接验证但服务未运行会直接失败，不再静默跳过后仍报告完成。设置写入完成与连接确认是不同层次；如果返回 `connection_unconfirmed`，保留现有身份并分别运行 `doctor --network` 和 `doctor --sunshine`，不能把它视为 Manager 与 Sunshine 均已连接，也不要删除状态重新配对。

当前 v2 有效身份无需重复初始化或配对。再次运行 `setup` 会复用身份，待处理事务会调用 `pair resume`；`config show --format json` 查看脱敏配置与修订，候选配置只支持 `sunshine_endpoint`；用 `config validate/diff/apply --file <绝对路径>`，提交还需要 `--expected-revision <当前修订>`，写入前停止服务。Client 不再询问 Sunshine 管理权限，配对后直接启用当前平台支持的专用能力。

若 `status` 返回 `pairing_state_incompatible`，不要重命名整个状态目录。先在 Manager 创建新实例授权码，再运行 `pair replace --interactive`。Client 会先只读验证执行日志；验证通过后将旧 `identity.json` 归档为唯一的 `identity.incompatible-*.json`，验证失败则返回 `important_state_incompatible`，且配对资料与执行日志均保持原样。

`--config` 选择整个状态目录（`--state` 是兼容别名）；系统服务操作应使用上面的默认目录。配对响应丢失使用 `status`、`pair status` 或 `pair resume`；不要用清空状态来重试。强制文件覆盖不会清除设备身份、凭据或执行记录。若检测到指向其他程序的同名服务、符号链接或不可信目录，安装会给出错误并保留数据；先检查对应路径，避免盲目删除。
