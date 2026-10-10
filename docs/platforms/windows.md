# 在 Windows 上使用 xscc

适用范围：Windows 11 x64。其他平台见[平台入口](../platform-setup.md)。

## 安装并配对

先完成[安装前准备](../platform-setup.md#开始前准备)。从 [1.0.0 Release](https://github.com/isarmg/xscc/releases/tag/v1.0.0) 下载 `xscc-1.0.0-windows-x64.msi` 与同名 `.sha256`。

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

向导各项输入与成功标准见[向导输入与完成结果](../platform-setup.md#向导输入与完成结果)。随后按[日常使用](../usage.md)核对服务、绑定和管理任务结果。

## 日志与本机状态

系统服务为 `Xscc`，运行账户为 LocalSystem。继续使用上面的 `$client` 路径读取日志：

```powershell
& $client service status --format json
& $client logs --tail 100 --format json
```

持久 JSON 日志默认每份 8 MiB，活动文件加四份归档共 40 MiB。`logs --follow --format ndjson --timeout 60s` 可有界跟踪；日志损坏或已过保留窗口会明确报错。

默认状态目录为 `C:\ProgramData\Xscc`；`--config` 指整个目录，系统服务使用默认位置。Sunshine 服务控制面向 `SunshineService`。

## 排查本机问题

- 找不到命令：重新打开终端，或按本页安装步骤读取 `InstallLocation`，使用 `$client` 调用。
- MSI 失败：打开 `$env:TEMP\xscc-install.log`，核对已有安装、服务路径和权限。退出码 3010 表示安装成功但需要重启。
- 服务启动失败：查看 SCM 服务退出码、系统事件和 `xscc logs`。远程 HTTPS 的证书必须受到实际服务账户信任。
- 管理端 TLS 失败：LocalSystem 使用计算机信任存储。Sunshine 401/403 和版本问题见[通用排障](../troubleshooting.md)。

需要修复时用已校验的同版 MSI 修复程序与服务文件；完成后检查版本、服务和业务连接。

## 卸载

在 Windows“已安装的应用”中卸载 xscc。普通卸载停止并移除系统服务，保留身份、凭据与执行记录；Sunshine 自己的安装不变。客户端没有自动清除业务状态的命令。

卸载完成后，对应系统服务应不再登记。

## 构建原生安装包

在原生 Windows x64 上准备 Rust `1.99.0` MSVC 工具链、MSVC C++ 构建工具、Python `3.11+` 和 .NET SDK；WiX `4.0.6` 由项目文件固定。从干净、已提交的仓库根目录运行，输出目录须为尚不存在的仓库外绝对路径：

```powershell
python scripts/package-client.py --output "$env:USERPROFILE\xscc-output"
```

生成 `xscc-1.0.0-windows-x64.msi`、`xscc-1.0.0-x86_64-pc-windows-msvc.zip` 及对应 manifest / 校验文件。

正式标签构建加 `--require-tag`，要求与版本号一致的 annotated tag 精确指向 HEAD。归档用于分发二进制与文档；系统服务通过原生安装包安装。

通用工具链、质量检查和源码说明见[开发指南](../development.md)。原生安装器构建成功后，还需在对应系统验证安装、服务生命周期和真实设备行为。

## 继续使用

[配置](../configuration.md) · [日常使用](../usage.md) · [维护](../administration.md) · [通用排障](../troubleshooting.md) · [命令参考](../cli-compatibility.md)
