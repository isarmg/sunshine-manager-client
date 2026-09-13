# 各平台安装、配置与覆盖升级

适用于 0.1.1。Client 是管理代理；需要先安装 Sunshine 并在其 Web UI 设置用户名、密码。Manager 地址、Manager 配对码、Sunshine 本机地址和 Sunshine 管理凭据是不同的输入。每个平台只发布一个原生安装包，安装器检查平台/架构/权限、注册服务，然后由 `setup` 完成配置。

## Windows 11 x64

下载并校验 Release 的 Windows MSI。在管理员 PowerShell 中进行普通安装；不要使用 `/qn`，MSI 提交部署后会直接启动交互式 `setup`：

```powershell
$client = "$env:ProgramFiles\SunshineClient\sunshine-client.exe"
msiexec.exe /i .\sunshine-client-0.1.1-windows-x64.msi /norestart
```

普通 MSI 安装提交后会启动 Setup 并请求一次 Windows 管理员确认；只有提权成功后才读取或写入受保护状态。若取消 UAC、使用 `/qn` 或其他无终端方式，安装器仍保留部署和服务登记；随后在管理员终端显式运行 `& "$env:ProgramFiles\SunshineClient\sunshine-client.exe" setup --interactive`。配对失败不会回滚已提交的安装。

交互输入 Manager 管理台生成的配对码、`https://127.0.0.1:47990/`、Sunshine Web UI 用户名和密码。默认服务账户 LocalSystem，状态目录 `C:\ProgramData\SunshineClient`。MSI 会把 `C:\Program Files\SunshineClient` 事务性追加到机器 PATH；新终端可直接运行 `sunshine-client`，卸载会移除该安装器拥有的 PATH 项。

已有版本直接再次运行同一 MSI；原生安装器处理升级、修复、降级检查和服务登记，保留设备身份、凭据、任务记录及启动意图。需要再次设置时运行 `sunshine-client setup` 会复用有效身份或恢复未完成事务，不会强制重新配对。

MSI 同版文件修复：`msiexec.exe /i "完整路径\sunshine-client-0.1.1-windows-x64.msi" REINSTALL=ALL REINSTALLMODE=amus /l*v "%TEMP%\sunshine-client-install.log"`（在 cmd 中执行）。安装器忙碌时等待其他安装结束；3010 表示 Windows 需要重启完成替换。使用 `Get-Service SunshineClient` 和 `sunshine-client service status` 检查服务。

## Ubuntu 24.04 x86_64

```sh
sudo apt install ./sunshine-client_0.1.1_amd64.deb
sudo sunshine-client setup
```

默认配置与状态位于 `/var/lib/sunshine-client`，运行账户 `sunshine-client`。安装后运行 `sudo sunshine-client setup`，按提示选择开机启动、立即启动并验证连接。日志：`sudo journalctl -u sunshine-client.service -n 100 --no-pager`。

新 DEB 可直接覆盖旧版并保留状态；同版损坏执行 `sudo apt install --reinstall ./sunshine-client_0.1.1_amd64.deb`。升级会停止旧进程并恢复原来处于运行状态的服务，不清除身份或任务去重记录。若安装器没有交互终端，稍后运行 `sudo sunshine-client setup`。

手工归档只包含可执行文件、文档和校验元数据，不包含 Shell/Python 安装包装脚本；生产安装和覆盖请使用 DEB。解压的可执行文件可用于临时诊断，但不替代原生服务安装器。

## macOS Apple Silicon

只提供 Apple Silicon 原生 PKG，不提供 Intel 版本：

```sh
sudo installer -pkg ./sunshine-client-0.1.1-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/sunshine-client setup
```

状态路径 `/Library/Application Support/sunshine-client`，系统服务账户 `_sunshineclient`；LaunchDaemon `org.sarmg.sunshine-client` 无需用户登录即可运行。诊断命令 `sudo launchctl print system/org.sarmg.sunshine-client`，日志 `/var/log/sunshine-client.log`。

再次运行原生 PKG 即可覆盖旧程序，或在保留状态的卸载后重装。安装器保留身份和执行记录；随后运行 `/usr/local/bin/sunshine-client setup`。发行文件未签名、未公证，使用系统提供的本地批准入口允许已校验的程序运行，不要全局关闭系统安全检查。

## Sunshine HTTPS 证书配置（两种互斥模式）

Client 始终验证 TLS，不提供 `--insecure`。配置 Sunshine 连接时必须明确选择以下一种模式：

1. **系统信任与名称验证**：不提交 `sunshine_certificate` 或 `sunshine_certificate_path`。证书链必须被运行
   Client 服务的系统账户信任，并且证书 SAN 必须匹配 `sunshine_endpoint` 的主机名或 IP。此模式适合由
   企业 CA 或公共 CA 签发的证书；只有 endpoint 使用 `127.0.0.1` 时才需要 `IP:127.0.0.1` SAN。
2. **Sunshine 证书精确固定**：在 `setup` 输入中提供 `sunshine_certificate_path`，指向 Sunshine 配置项
   `cert` 对应的公开 PEM（通常是 Sunshine 自带的 `cacert.pem`）；自动化也可直接提供
   `sunshine_certificate` PEM 字符串，两者不能同时存在。Client 保存公开证书并要求服务端呈现完全相同的
   证书，不依赖系统 CA，也不要求回环 IP SAN。路径必须是本机绝对路径、普通文件且不能是符号链接，私钥
   `pkey` 绝不能提供给 Client。

浏览器点击“继续访问”只影响浏览器，不会改变后台服务的系统信任，也不会建立证书固定。若采用系统信任模式，
核对签发 CA 的来源和 SHA256 指纹后，可将 **CA 公共证书** 导入系统信任库：

公开的首次配对入口使用受保护的 stdin JSON；字段与内部 `deploy/bootstrap.example.json` 不同：

```json
{"server":"https://manager.example.org/","authorization_code":"REPLACE_WITH_INSTANCE_CODE","sunshine_endpoint":"https://127.0.0.1:47990/","sunshine_certificate_path":"/absolute/path/to/cacert.pem","sunshine_username":"REPLACE_LOCALLY","sunshine_password":"REPLACE_LOCALLY","restart_allowed":false}
```

此 JSON 只可交给 `sunshine-client setup --input-stdin --non-interactive` 的 stdin。占位符不能直接使用；证书
路径属于 Client 主机；只提供 Sunshine 的公开证书，绝不提供 `pkey` 私钥；秘密不得放进命令参数、部署日志
或版本库。`deploy/bootstrap.example.json` 仅供旧的受保护 bootstrap 导入入口使用。

Windows 管理员 PowerShell：

```powershell
Import-Certificate -FilePath .\sunshine-local-ca.cer -CertStoreLocation Cert:\LocalMachine\Root
```

Ubuntu（PEM 格式公共证书，扩展名 `.crt`）：

```sh
sudo install -m 0644 ./sunshine-local-ca.crt /usr/local/share/ca-certificates/sunshine-local-ca.crt
sudo update-ca-certificates
```

macOS：

```sh
sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ./sunshine-local-ca.cer
```

完成后使用管理员终端执行 `sunshine-client doctor --sunshine`，再运行 `sunshine-client setup`。系统信任模式
发生名称不匹配时，导入 CA 无法修复，必须让证书 SAN 与 endpoint 一致。Sunshine 更换证书后，精确固定模式
会按设计拒绝连接；停止 Client 服务并核对新证书来源和指纹，然后运行
`sunshine-client credentials update --interactive`，输入新用户名、密码和 `cacert.pem` 绝对路径。自动化输入
可增加 `sunshine_certificate_path` 或 `sunshine_certificate`；显式设置 `use_system_trust: true` 会移除旧固定证书，
且不能同时提供证书字段。留空证书更新会保留现有信任模式。此操作不重新配对 Manager，但 Sunshine 本身若
更换了设备配对状态，仍需按 Sunshine/Moonlight 流程确认。

## 已有配置、升级与故障处理

`setup` 按配置、配对、服务注册、启动策略、运行状态和连接顺序执行后置验证。交互终端会逐步显示 `verified`；JSON 失败响应中的 `error.step` 指明失败关卡，`error.code` 和 `error.message` 给出稳定原因，操作系统服务命令失败时 `error.detail` 保留经过控制字符清理和长度限制的原始诊断。请求连接验证但服务未运行会直接失败，不再静默跳过后仍报告完成。设置写入完成与连接确认是不同层次；如果返回 `connection_unconfirmed`，保留现有身份并分别运行 `doctor --network` 和 `doctor --sunshine`，不能把它视为 Manager 与 Sunshine 均已连接，也不要删除状态重新配对。

已有有效身份无需重复初始化或配对。再次运行 `setup` 会复用身份，待处理事务会调用 `pair resume`；`config show --format json` 查看脱敏配置与修订，候选配置只支持 `sunshine_endpoint`、`restart_allowed`；用 `config validate/diff/apply --file <绝对路径>`，提交还需要 `--expected-revision <当前修订>`，写入前停止服务。

`--config` 选择整个状态目录（`--state` 是兼容别名）；系统服务操作应使用上面的默认目录。配对响应丢失使用 `status`、`pair status` 或 `pair resume`；不要用清空状态来重试。强制文件覆盖不会清除设备身份、凭据或执行记录。若检测到指向其他程序的同名服务、符号链接或不可信目录，安装会给出错误并保留数据；先检查对应路径，避免盲目删除。
