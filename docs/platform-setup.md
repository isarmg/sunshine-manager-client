# 安装并配对 xscc

本页可按所用平台直接操作。准备 [xscs 管理员](https://github.com/isarmg/xscs/blob/main/docs/instance-management.md)提供的 HTTPS 根地址和实例授权码，使用管理员终端完成安装与设置。

先安装 Sunshine `v2026.914.233613`，在其 Web UI 设置用户名和密码。准备本机 HTTPS 回环地址，通常是 `https://127.0.0.1:47990/`。这个密码用于本机 Sunshine API，实例授权码用于远程管理端。Sunshine 上游安装说明见[Getting Started](https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2getting__started.html)。

从 [1.0.0 Release](https://github.com/isarmg/xscc/releases/tag/v1.0.0)下载本机原生安装包及对应 `.sha256`。先核对摘要，再安装。以下文件名对应该发行页。

| 平台 | 原生包 | 服务与账户 |
|---|---|---|
| Windows 11 x64 | `xscc-1.0.0-windows-x64.msi` | `Xscc` / LocalSystem |
| Ubuntu 24.04 x86_64 | `xscc_1.0.0_amd64.deb` | `xscc.service` / `xscc` |
| macOS Apple Silicon | `xscc-1.0.0-macos-arm64-unsigned.pkg` | `org.sarmg.xscc` / `_xscc` |

Linux 原生构建与验证基线为 Ubuntu 24.04。macOS PKG 未签名、未公证，核验来源与摘要后，按系统提供的批准流程安装。

## Windows

在管理员 PowerShell 的下载目录中计算 SHA-256，与校验文件内同名行比较：

```powershell
Get-FileHash .\xscc-1.0.0-windows-x64.msi -Algorithm SHA256
```

一致后安装并等待向导完成：

```powershell
$msi = (Resolve-Path .\xscc-1.0.0-windows-x64.msi).Path
$install = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /norestart /l*v `"$env:TEMP\xscc-install.log`"" -Wait -PassThru
$install.ExitCode
```

退出码 0 表示成功，3010 表示成功但需重启；其他代码查看安装日志。安装完成后使用实际安装路径启动向导：

```powershell
$installRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\xscc').InstallLocation
$client = Join-Path $installRoot 'xscc.exe'
& $client version --format json
& $client service stop
& $client setup --interactive
& $client status --check --format json
& $client doctor --sunshine --format json
```

MSI 允许选择本机安装目录，安装全部组件并登记系统服务；安装事务结束后再配对。新终端可直接使用命令名。

## Linux

进入下载目录，先核对安装包摘要：

```sh
sha256sum --check --strict xscc_1.0.0_amd64.deb.sha256
```

预期校验结果为 OK。随后执行：

```sh
sudo apt install ./xscc_1.0.0_amd64.deb
sudo xscc service stop
sudo xscc setup --interactive
sudo xscc status --check --format json
sudo xscc doctor --sunshine --format json
```

## macOS

在下载目录核对 PKG 摘要，与对应 `.sha256`中同名行比较：

```sh
shasum -a 256 xscc-1.0.0-macos-arm64-unsigned.pkg
```

一致后安装并设置：

```sh
sudo installer -pkg ./xscc-1.0.0-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc setup --interactive
sudo /usr/local/bin/xscc status --check --format json
sudo /usr/local/bin/xscc doctor --sunshine --format json
```

macOS 的 Sunshine 服务启停由本机管理；xscc 可使用其管理 API。

## 向导输入与完成结果

向导依次询问管理端地址、实例授权码、本机 Sunshine 回环 HTTPS 地址、用户名和密码。首次设置会创建缺少的配置，随后询问开机自启（默认 Yes），启动服务并验证连接。

授权码在终端中明文回显，Sunshine 密码隐藏输入；请在受保护终端输入。将 HTTPS 地址填为根地址，例如 `https://manager.example.com`。远程证书应被服务账户的系统信任库信任且匹配域名。

成功后到 xscs 管理台确认设备在线，读取一项配置或状态并看到任务结果。配对中断时先查看 `xscc pair status --format json`，再用 `pair resume` 继续现有事务。

## 后续维护与卸载

服务启停、配置修改和Sunshine 密码更新见[日常维护](https://github.com/isarmg/xscc/blob/main/docs/administration.md)；连接问题见[排查问题](https://github.com/isarmg/xscc/blob/main/docs/troubleshooting.md)。

普通卸载停止并移除程序和服务，保留身份、凭据与执行记录：

- Windows：在“已安装的应用”中卸载 xscc。
- Debian/Ubuntu：`sudo apt remove xscc`。
- macOS：取得与所装版本匹配的源码，从仓库根执行 `sudo sh deploy/macos/uninstall-macos.sh`。该脚本未随 PKG 安装。

卸载后系统服务应不再登记。Sunshine 保留自己的安装和服务状态。当前没有自动清除业务状态的命令。
