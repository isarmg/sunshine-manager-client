# 各平台安装、配置与覆盖升级

适用于 0.1.0-rc.7。Client 是管理代理；需要先安装 Sunshine 并在其 Web UI 设置用户名、密码。Manager 地址、Manager 配对码、Sunshine 本机地址和 Sunshine 管理凭据是不同的输入。每个平台只发布一个原生安装包，安装器检查平台/架构/权限、注册服务，然后由 `setup` 完成配置。

## Windows 11 x64

下载并校验 Release 的 Windows MSI。在管理员 PowerShell 中进行普通安装；不要使用 `/qn`，MSI 提交部署后会直接启动交互式 `setup`：

```powershell
$client = "$env:ProgramFiles\SunshineClient\sunshine-client.exe"
msiexec.exe /i .\sunshine-client-0.1.0-rc.7-windows-x64.msi /norestart
```

若使用 `/qn` 或其他无终端方式，安装器只完成部署和服务登记；随后在管理员终端显式运行 `& "$env:ProgramFiles\SunshineClient\sunshine-client.exe" setup --interactive`。配对失败不会回滚已提交的安装。

交互输入 Manager 管理台生成的配对码、`https://127.0.0.1:47990/`、Sunshine Web UI 用户名和密码。默认服务账户 LocalSystem，状态目录 `C:\ProgramData\SunshineClient`；无需配置用户 PATH。

已有版本直接再次运行同一 MSI；原生安装器处理升级、修复、降级检查和服务登记，保留设备身份、凭据、任务记录及启动意图。需要再次设置时运行 `sunshine-client setup` 会复用有效身份或恢复未完成事务，不会强制重新配对。

MSI 同版文件修复：`msiexec.exe /i "完整路径\sunshine-client-0.1.0-rc.7-windows-x64.msi" REINSTALL=ALL REINSTALLMODE=amus /l*v "%TEMP%\sunshine-client-install.log"`（在 cmd 中执行）。安装器忙碌时等待其他安装结束；3010 表示 Windows 需要重启完成替换。使用 `Get-Service SunshineClient` 和 `sunshine-client service status` 检查服务。

## Ubuntu 24.04 x86_64

```sh
sudo apt install ./sunshine-client_0.1.0-rc.7_amd64.deb
sudo sunshine-client setup
```

默认配置与状态位于 `/var/lib/sunshine-client`，运行账户 `sunshine-client`。安装后运行 `sudo sunshine-client setup`，按提示选择开机启动、立即启动并验证连接。日志：`sudo journalctl -u sunshine-client.service -n 100 --no-pager`。

新 DEB 可直接覆盖旧版并保留状态；同版损坏执行 `sudo apt install --reinstall ./sunshine-client_0.1.0-rc.7_amd64.deb`。升级会停止旧进程并恢复原来处于运行状态的服务，不清除身份或任务去重记录。若安装器没有交互终端，稍后运行 `sudo sunshine-client setup`。

手工归档只包含可执行文件、文档和校验元数据，不包含 Shell/Python 安装包装脚本；生产安装和覆盖请使用 DEB。解压的可执行文件可用于临时诊断，但不替代原生服务安装器。

## macOS Apple Silicon

只提供 Apple Silicon 原生 PKG，不提供 Intel 版本：

```sh
sudo installer -pkg ./sunshine-client-0.1.0-rc.7-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/sunshine-client setup
```

状态路径 `/Library/Application Support/sunshine-client`，系统服务账户 `_sunshineclient`；LaunchDaemon `org.sarmg.sunshine-client` 无需用户登录即可运行。诊断命令 `sudo launchctl print system/org.sarmg.sunshine-client`，日志 `/var/log/sunshine-client.log`。

再次运行原生 PKG 即可覆盖旧程序，或在保留状态的卸载后重装。安装器保留身份和执行记录；随后运行 `/usr/local/bin/sunshine-client setup`。发行文件未签名、未公证，使用系统提供的本地批准入口允许已校验的程序运行，不要全局关闭系统安全检查。

## Sunshine HTTPS 证书配置（每个平台都需要）

Client 只接受 HTTPS 回环 IP 字面量，例如 `https://127.0.0.1:47990/`，证书 SAN 必须包含 `IP:127.0.0.1`。浏览器点击“继续访问”只影响浏览器，不会为系统服务建立信任。

若 Sunshine 默认证书不满足要求，请由管理员准备带正确 IP SAN 的证书及私钥，在 Sunshine 配置中设置 `cert` 和 `pkey` 的绝对路径并重启 Sunshine。配置项及密钥兼容要求见 [Sunshine 官方配置说明](https://docs.lizardbyte.dev/projects/sunshine/master/md_docs_2configuration.html)。这会更改 Sunshine 的服务器证书，现有 Moonlight 配对可能需要重新确认；Client 安装器不会自动替换它。证书信任应在运行 Client 服务的账户/系统范围完成后再运行 `setup`。

核对签发 CA 的来源和 SHA256 指纹后，将 **CA 公共证书** 导入系统信任库，私钥不导入、不发送：

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

完成后使用管理员终端执行 `sunshine-client doctor --sunshine`，再运行 `sunshine-client setup`。名称不匹配时，导入 CA 也无法修复，必须换成带正确 IP SAN 的服务器证书。认证失败时在停止 Client 服务后运行 `sunshine-client credentials update --interactive`，输入 Sunshine 的用户名和密码。

## 已有配置、升级与故障处理

已有有效身份无需重复初始化或配对。再次运行 `setup` 会复用身份，待处理事务会调用 `pair resume`；`config show --format json` 查看脱敏配置与修订，候选配置只支持 `sunshine_endpoint`、`restart_allowed`；用 `config validate/diff/apply --file <绝对路径>`，提交还需要 `--expected-revision <当前修订>`，写入前停止服务。

`--config` 选择整个状态目录（`--state` 是兼容别名）；系统服务操作应使用上面的默认目录。配对响应丢失使用 `status`、`pair status` 或 `pair resume`；不要用清空状态来重试。强制文件覆盖不会清除设备身份、凭据或执行记录。若检测到指向其他程序的同名服务、符号链接或不可信目录，安装会给出错误并保留数据；先检查对应路径，避免盲目删除。
