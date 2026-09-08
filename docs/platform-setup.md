# 各平台安装、配置与覆盖升级

适用于 0.1.0-rc.6。Client 是管理代理；需要先安装 Sunshine 并在其 Web UI 设置用户名、密码。Manager 地址、Manager 配对码、Sunshine 本机地址和 Sunshine 管理凭据是不同的输入。安装与覆盖不会重置它们。

## Windows 11 x64

下载 Release 的 Windows ZIP，校验旁边的 SHA256 文件后解压。在管理员 PowerShell 中进入解压目录：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1 -Binary "$PWD\sunshine-client.exe"
$client = "$env:ProgramFiles\SunshineClient\sunshine-client.exe"
& $client config init --interactive
& $client pair --server https://manager.example.com --interactive
& $client doctor --sunshine
& $client service enable --now
& $client status
```

交互输入 Manager 管理台生成的配对码、`https://127.0.0.1:47990/`、Sunshine Web UI 用户名和密码。默认服务账户 LocalSystem，状态目录 `C:\ProgramData\SunshineClient`；无需配置用户 PATH。

首次也可双击 MSI 安装，再在管理员终端执行上述配置命令。已有版本推荐运行 ZIP 中的安装脚本覆盖：它验证同名服务的程序路径和运行账户，停止服务、保存旧程序、强制覆盖文件，成功后恢复之前运行状态；失败会恢复程序并打印受保护的备份目录。已有状态不重新导入 Bootstrap。不要混用不同版本的 ZIP 文件。

MSI 同版文件修复：`msiexec.exe /i "完整路径\sunshine-client-0.1.0-rc.6-windows-x64.msi" REINSTALL=ALL REINSTALLMODE=amus /l*v "%TEMP%\sunshine-client-install.log"`（在 cmd 中执行）。安装器忙碌时等待其他安装结束；3010 表示 Windows 需要重启完成替换。使用 `Get-Service SunshineClient` 和 `sunshine-client service status` 检查服务。

## Ubuntu 24.04 x86_64

```sh
sudo apt install ./sunshine-client_0.1.0-rc.6_amd64.deb
sudo sunshine-client config init --interactive
sudo sunshine-client pair --server https://manager.example.com --interactive
sudo sunshine-client doctor --sunshine
sudo sunshine-client service enable --now
sudo sunshine-client status
```

默认配置与状态位于 `/var/lib/sunshine-client`，运行账户 `sunshine-client`。初装不自动启动；完成配对后启用 systemd 服务。日志：`sudo journalctl -u sunshine-client.service -n 100 --no-pager`。

新 DEB 可直接覆盖旧版并保留状态；同版损坏执行 `sudo apt install --reinstall ./sunshine-client_0.1.0-rc.6_amd64.deb`。升级会停止旧进程并恢复原来处于运行状态的服务，不清除身份或任务去重记录。

使用 tar.gz 手工安装时，解压并进入目录，运行 `sudo bash ./install-linux.sh "$PWD/sunshine-client"`。重复运行可修复程序与服务文件，卸载保留状态后也可重装。它会识别已有 DEB 的服务路径，避免创建覆盖包服务的旧 `/etc/systemd/system` 副本。不要在修复过程中修改服务账户 UID 或状态目录归属。

## macOS Apple Silicon

只提供 `aarch64-apple-darwin` 的 tar.gz，不提供 Intel 版本。解压并进入目录：

```sh
sudo sh ./install-macos.sh "$PWD/sunshine-client"
sudo /usr/local/bin/sunshine-client config init --interactive
sudo /usr/local/bin/sunshine-client pair --server https://manager.example.com --interactive
sudo /usr/local/bin/sunshine-client doctor --sunshine
sudo /usr/local/bin/sunshine-client service enable --now
sudo /usr/local/bin/sunshine-client status
```

状态路径 `/Library/Application Support/sunshine-client`，系统服务账户 `_sunshineclient`；LaunchDaemon `org.sarmg.sunshine-client` 无需用户登录即可运行。诊断命令 `sudo launchctl print system/org.sarmg.sunshine-client`，日志 `/var/log/sunshine-client.log`。

再次运行新包的安装脚本即可覆盖旧程序，或在保留状态的卸载后重装。活动服务会短暂停止并重新加载；未启动的服务保持未启动。发行文件未签名、未公证，使用系统提供的本地批准入口允许已校验的程序运行，不要全局关闭系统安全检查。

## Sunshine HTTPS 证书配置（每个平台都需要）

Client 只接受 HTTPS 回环 IP 字面量，例如 `https://127.0.0.1:47990/`，证书 SAN 必须包含 `IP:127.0.0.1`。浏览器点击“继续访问”只影响浏览器，不会为系统服务建立信任。

若 Sunshine 默认证书不满足要求，请由管理员准备带正确 IP SAN 的证书及私钥，在 Sunshine 配置中设置 `cert` 和 `pkey` 的绝对路径并重启 Sunshine。配置项及密钥兼容要求见 [Sunshine 官方配置说明](https://docs.lizardbyte.dev/projects/sunshine/master/md_docs_2configuration.html)。这会更改 Sunshine 的服务器证书，现有 Moonlight 配对可能需要重新确认；Client 安装器不会自动替换它。

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

完成后使用管理员终端执行 `sunshine-client doctor --sunshine`，再启用服务。名称不匹配时，导入 CA 也无法修复，必须换成带正确 IP SAN 的服务器证书。认证失败时在停止 Client 服务后运行 `sunshine-client credentials update --interactive`，输入 Sunshine 的用户名和密码。

## 已有配置、升级与故障处理

已有有效身份无需重复 `config init` 或 `pair`。`config show --format json` 查看脱敏配置与修订，候选配置只支持 `sunshine_endpoint`、`restart_allowed`；用 `config validate/diff/apply --file <绝对路径>`，提交还需要 `--expected-revision <当前修订>`，写入前停止服务。

`--config` 选择整个状态目录（`--state` 是兼容别名）；系统服务操作应使用上面的默认目录。配对响应丢失使用 `pair status`、`pair resume`；不要用清空状态来重试。强制文件覆盖不会清除设备身份、凭据或执行记录。若检测到指向其他程序的同名服务、符号链接或不可信目录，安装会给出错误并保留数据；先检查对应路径，避免盲目删除。
