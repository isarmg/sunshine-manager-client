# xscc 分平台部署与维护

适用于 `xscc 1.0.0`。本文按 Windows、Ubuntu、macOS 分别给出安装、首次配对、重新配对、服务查看、启停、诊断、升级和卸载步骤。配置字段和受保护 stdin 自动化见[配置指南](configuration.md)；本文末尾保留原生安装器与协议边界的详细说明。

## 开始前的准备

1. 从 [Client Releases](https://github.com/isarmg/xscc/releases) 下载目标版本、本机平台的原生包和校验文件。文件名以 1.0.0 为例。
2. 在同一台机器上先安装受支持的 Sunshine `v2026.914.233613`，并在 Sunshine Web UI 设置用户名、密码。Client 不负责安装 Sunshine。
3. 请 Manager 管理员创建实例并提供 Manager HTTPS 根地址和实例授权码。Manager 授权码与 Sunshine 密码用途不同，不能互换。
4. Sunshine 本机管理地址使用 `https://127.0.0.1:47990/`，或实际使用的回环 HTTPS 端口；不接受局域网 IP、主机名、HTTP 和额外路径。
5. 使用管理员终端。命令块中的注释解释命令用途；按需操作的命令组不能全部依次执行。授权码在交互终端中明文回显，Sunshine 密码隐藏输入。

| 平台 | Client 系统服务/账户 | 默认状态目录 | 被管理的 Sunshine 服务 |
|---|---|---|---|
| Windows 11 x64 | `Xscc` / LocalSystem | `C:\ProgramData\Xscc` | `SunshineService` |
| Ubuntu 24.04 x86_64 | `xscc.service` / `xscc` | `/var/lib/xscc` | 系统级 `sunshine.service` |
| macOS Apple Silicon | `org.sarmg.xscc` / `_xscc` | `/Library/Application Support/xscc` | 当前不提供服务控制能力 |

`xscc service ...` 管理的是 Client 代理。停止代理后 Manager 将无法下发新的本机管理任务；已安装的 Sunshine 有自己的服务状态。`--config` 选择整个状态目录，系统服务操作使用默认目录。

## Windows 11 x64

### 1. 安装与确认

管理员 PowerShell 中执行，对比校验值后才安装：

```powershell
# 切换到安装包所在目录。
Set-Location "$env:USERPROFILE\Downloads"
# 计算 MSI 哈希，对比同版 Release 校验文件中该文件的行。
Get-FileHash .\xscc-1.0.0-windows-x64.msi -Algorithm SHA256
# 等待安装向导结束，不自动重启，并保存安装日志。
$msi = (Resolve-Path .\xscc-1.0.0-windows-x64.msi).Path
$install = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /norestart /l*v `"$env:TEMP\xscc-install.log`"" -Wait -PassThru
# 0 为成功；3010 为成功但需重启；其他码先查安装日志。
$install.ExitCode
```

成功后取得实际安装路径。MSI 全部安装，无功能选择页；安装事务内不运行交互配对。

```powershell
# 读取用户在 MSI 中选择的安装目录，避免依赖当前终端尚未刷新的 PATH。
$installRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\xscc').InstallLocation
$client = Join-Path $installRoot 'xscc.exe'
# & 执行变量中的程序；确认版本及原生服务登记。
& $client version --format json
& $client service status --format json
```

### 2. 首次配对、启动与验收

```powershell
# 停止可能运行的旧代理，避免配置和身份写入与后台进程并发。
& $client service stop
# 完整向导输入 Manager 地址/授权码、Sunshine 回环地址及其用户名/密码。
& $client setup --interactive
# 分别查看本机绑定、系统服务状态和业务检查结果。
& $client pair status --format json
& $client service status --format json
& $client status --check --format json
# 实际访问本机 Sunshine API，验证凭据和受支持版本。
& $client doctor --sunshine --format json
```

开机自启提示默认 Yes；向导随后固定启动服务并验证连接。首次不必手动 `config init`。验收还需在 Manager 确认设备在线，执行一项只读任务并看到结果；不要仅凭本机配对文件判断通信成功。

### 3. 重新配对与更新本机密码

Manager 轮换授权码、旧凭据失效或账户格式不兼容时，先取得新码，再执行：

```powershell
# 停止代理并核对旧配对状态。
& $client service stop
& $client pair status --format json
# 显式替换 Manager 绑定，输入新授权码及本机 Sunshine 凭据；保留安装身份和执行记录。
& $client pair replace --interactive
# 查看保存结果，再启动并验证两条连接。
& $client pair status --format json
& $client service start
& $client doctor --network --format json
& $client doctor --sunshine --format json
```

仅配对请求中断时，先 `& $client pair resume --interactive`；再次普通 `setup` 会复用有效身份或恢复事务。只更改了 Sunshine 用户名/密码时使用下面流程，无需更换 Manager 绑定：

```powershell
# 停服后更新本机 Sunshine 的管理凭据，保留 Manager 身份和任务记录。
& $client service stop
& $client credentials update --interactive
# 验证新凭据可用，再恢复服务。
& $client doctor --sunshine --format json
& $client service start
```

### 4. 服务查看、启停与自启

以下为独立操作，按需选择：

```powershell
# 查看 Client 服务状态、开机策略与登记路径。
& $client service status --format json
Get-Service -Name Xscc
sc.exe qc Xscc
# 立即启动、停止、重启代理；不改变开机策略。
& $client service start
& $client service stop
& $client service restart
# 分别设为自动/手动启动，不改变当前运行状态。
& $client service enable
& $client service disable
# 同时设为手动并立即停止代理。
& $client service disable --now
# 只读查看 Sunshine 自己的服务状态，帮助区分代理和被管理程序。
Get-Service -Name SunshineService
```

### 5. 诊断

```powershell
# 查看脱敏配置与业务状态。
& $client config show --format json
& $client status --check --format json
# 两项检查分别定位远程 Manager 网络入口与本机 Sunshine API。
& $client doctor --network --format json
& $client doctor --sunshine --format json
# 查看最近 100 条后台持久日志与本地任务记录。
& $client logs --tail 100 --format json
& $client tasks list --format json
```

失败先记录脱敏的 `error.step`、`error.code` 和 `error.detail`；`connection_unconfirmed` 表示设置可能已保存、连接尚未确认。服务尚未建立日志时，查看事件查看器的 System/Service Control Manager 事件及 `sc.exe query Xscc` 返回的退出信息。

### 6. 升级、修复与卸载

本章后续维护若在新 PowerShell 会话中进行，先重新执行第 1 步读取注册表的 `$installRoot` / `$client` 两行。涉及修复或卸载时，再用 `$msi = (Resolve-Path .\xscc-1.0.0-windows-x64.msi).Path` 指向与已安装产品相匹配的 MSI；文件在其他目录时用其实际完整路径。不要沿用指向另一版本包的变量。

校验新版 MSI 后重复安装流程。MSI 升级会重新登记 Automatic 服务，升级后核对是否需要取消自启。当前版文件修复：

```powershell
# 等待 MSI 重装所有组件，保留身份、凭据和任务记录，并写入修复日志。
$repair = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" REINSTALL=ALL REINSTALLMODE=amus /norestart /l*v `"$env:TEMP\xscc-repair.log`"" -Wait -PassThru
$repair.ExitCode
```

正式退役先在 Manager 撤销设备，核对没有结果未确认的任务并保全执行记录。然后卸载：

```powershell
# 移除 Client 程序、服务和安装器的 PATH 项，保留身份、凭据与执行记录。
$remove = Start-Process msiexec.exe -ArgumentList "/x `"$msi`" /norestart /l*v `"$env:TEMP\xscc-uninstall.log`"" -Wait -PassThru
$remove.ExitCode
# 应找不到 Client 服务；Sunshine 本身继续按其自己的安装方式管理。
Get-Service -Name Xscc -ErrorAction SilentlyContinue
```

也可使用系统“已安装的应用”卸载。本产品 MSI 没有受支持的 `PURGE=1` 业务清理流程；保留目录不等于卸载失败，不要为了清理空间删除未解决任务的执行记录。

## Ubuntu 24.04 x86_64

### 1. 安装

```sh
# 确认 CPU 架构并对比同版校验文件中的 DEB 哈希。
uname -m
sha256sum ./xscc_1.0.0_amd64.deb
# APT 安装本地 DEB 并解决依赖，登记系统级服务。
sudo apt install ./xscc_1.0.0_amd64.deb
# 确认软件版本及 Client 服务登记。
xscc version --format json
sudo xscc service status --format json
```

DEB 在具备交互终端时可能已经调用 `setup`。若已完成配对，直接验收；若未完成或没有终端，执行下一步。生产安装使用 DEB；手工二进制归档用于诊断，不提供系统服务安装器。

### 2. 首次设置与验收

```sh
# 停止旧代理，独占执行配置和身份写入。
sudo xscc service stop
# 输入 Manager 地址/授权码、Sunshine 回环地址和用户名/密码，自动启动并验证。
sudo xscc setup --interactive
# 核对配对、系统服务及业务状态。
sudo xscc pair status --format json
sudo xscc service status --format json
sudo xscc status --check --format json
# 验证 Sunshine 版本和认证，核对开机自启策略。
sudo xscc doctor --sunshine --format json
systemctl is-enabled xscc.service
```

`setup` 自启默认 Yes。Manager 管理页显示设备在线且只读任务返回结果后，再进行其他管理操作。

### 3. 重新配对、本机密码更新

```sh
# 停止代理并读取旧绑定。
sudo xscc service stop
sudo xscc pair status --format json
# 轮换 Manager 授权码后用新码替换绑定；保留安装身份与执行记录。
sudo xscc pair replace --interactive
# 核对新绑定，恢复代理并验证 Manager 和 Sunshine。
sudo xscc pair status --format json
sudo xscc service start
sudo xscc doctor --network --format json
sudo xscc doctor --sunshine --format json
```

请求中断先 `sudo xscc pair resume --interactive`；只更改本机 Sunshine 密码时，停服后执行 `sudo xscc credentials update --interactive`，随后 `doctor --sunshine`，成功后 `service start`。`important_state_incompatible` 必须保全执行日志，不能删目录重新配对。

### 4. 服务管理

按需单独执行：

```sh
# 原生状态显示 unit、运行状态与最近日志；q 退出分页。
systemctl status xscc.service
# CLI 返回服务登记、运行状态、开机策略。
sudo xscc service status --format json
# 立即启动、停止、重启 Client 代理。
sudo xscc service start
sudo xscc service stop
sudo xscc service restart
# 开机自启开关；不改变当前运行状态。
sudo xscc service enable
sudo xscc service disable
# 取消自启并立即停止。
sudo xscc service disable --now
# 查看被管理的 Sunshine 系统服务，区别于 Client 代理。
systemctl status sunshine.service
```

### 5. 诊断

```sh
# 分别检查 Manager 网络和 Sunshine 认证、版本。
sudo xscc doctor --network --format json
sudo xscc doctor --sunshine --format json
# 查看 Client 最近 100 条日志，不分页；-f 可跟踪新日志，Ctrl+C 退出查看。
sudo journalctl -u xscc.service -n 100 --no-pager
sudo journalctl -u xscc.service -f
# 查看任务记录，定位已执行、待回报或失败的任务。
sudo xscc tasks list --format json
```

Sunshine 服务不可用时检查 `sunshine.service` 自身的日志及 Web UI。本机 Sunshine 默认自签证书按严格回环策略处理；远程 Manager 始终需要可信证书，不要把两者的 TLS 策略混用。

### 6. 升级、修复、卸载

```sh
# 同版重装修复程序和服务文件；新版升级改用新版 DEB 路径。
sudo apt install --reinstall ./xscc_1.0.0_amd64.deb
# 普通卸载，停止并移除 Client 服务和包文件，保留身份与执行日志。
sudo apt remove xscc
# 确认 unit 已不再 loaded；systemd 可能仍保留旧的失败记录。
systemctl show xscc.service --property=LoadState,ActiveState
```

退役前在 Manager 撤销设备并保全未解决任务记录。即便 `apt purge`，当前包维护脚本仍保留业务状态和专用账户；没有自动删除执行记录的清理命令。原脚本安装的旧部署使用仓库 `deploy/uninstall-linux.sh`，它检查固定 unit/程序所有权并保留状态；DEB 安装应通过 APT 卸载，不要混用手工脚本。

## macOS Apple Silicon

### 1. 安装

只支持 arm64 Mac。下载未签名、未公证的原生 PKG，校验后通过系统批准入口安装。

```sh
# 确认 arm64 架构，并计算 PKG 的 SHA-256 供对照校验文件。
uname -m
shasum -a 256 ./xscc-1.0.0-macos-arm64-unsigned.pkg
# 安装系统 LaunchDaemon 和专用账户。
sudo installer -pkg ./xscc-1.0.0-macos-arm64-unsigned.pkg -target /
# 确认程序版本和服务登记；绝对路径避免 PATH 差异。
/usr/local/bin/xscc version --format json
sudo /usr/local/bin/xscc service status --format json
```

### 2. 首次配对、启动与验收

```sh
# 停止旧代理，再运行包含配对、自启、启动和连接验证的完整向导。
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc setup --interactive
# 核对本机绑定、服务运行和业务状态。
sudo /usr/local/bin/xscc pair status --format json
sudo /usr/local/bin/xscc service status --format json
sudo /usr/local/bin/xscc status --check --format json
# 验证 Sunshine 本机接口的版本与用户名/密码。
sudo /usr/local/bin/xscc doctor --sunshine --format json
```

自启提示默认 Yes。到 Manager 确认设备在线并执行只读任务。macOS 当前不声明 Sunshine 服务控制能力，不能据此要求 Manager 远程启停 Sunshine。

### 3. 重新配对与更新本机密码

```sh
# 停服并查看已有配对，再用新 Manager 授权码替换绑定。
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc pair status --format json
sudo /usr/local/bin/xscc pair replace --interactive
# 验证新绑定，恢复代理并确认 Manager 和 Sunshine 网络接口。
sudo /usr/local/bin/xscc pair status --format json
sudo /usr/local/bin/xscc service start
sudo /usr/local/bin/xscc doctor --network --format json
sudo /usr/local/bin/xscc doctor --sunshine --format json
```

请求中断使用 `pair resume --interactive`；仅 Sunshine 密码变化，停服后使用 `credentials update --interactive`，验证本机接口后启动代理。命令前缀均为 `sudo /usr/local/bin/xscc`。

### 4. 服务查看和启停

按需单独执行：

```sh
# 查看 CLI 服务状态及 launchd 的进程、最后退出信息。
sudo /usr/local/bin/xscc service status --format json
sudo launchctl print system/org.sarmg.xscc
# 立即启动、停止、重启代理；CLI 负责 launchd 装载/卸载。
sudo /usr/local/bin/xscc service start
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc service restart
# 设置/取消开机自启，不改变当前运行状态。
sudo /usr/local/bin/xscc service enable
sudo /usr/local/bin/xscc service disable
# 取消自启并立即停止当前进程。
sudo /usr/local/bin/xscc service disable --now
```

停止后 `launchctl print` 可能因 job 已卸载而报找不到服务；CLI 仍可确认 plist 已安装和停止状态。不要只杀进程，launchd 可能重新启动它。

### 5. 诊断

```sh
# 分别诊断 Manager 的远程入口和 Sunshine 的本机管理 API。
sudo /usr/local/bin/xscc doctor --network --format json
sudo /usr/local/bin/xscc doctor --sunshine --format json
# 查看最近 100 条日志；实时查看用 -f，Ctrl+C 只结束查看。
sudo tail -n 100 /var/log/xscc.log
sudo tail -f /var/log/xscc.log
# PKG 失败时查看系统安装日志；查看任务记录辅助判断任务是否已执行。
sudo tail -n 100 /var/log/install.log
sudo /usr/local/bin/xscc tasks list --format json
```

### 6. 升级、卸载

重新安装同版/新版已校验 PKG 即可修复或升级，保留身份、凭据与执行记录。卸载助手**未随 PKG 安装到机器上**；先取得与所装版本相匹配的仓库源码，从源码根目录执行：

```sh
# 运行仓库提供的 macOS 卸载脚本，核对程序/服务所有权后移除它们，保留身份、任务日志、账户和运行日志。
sudo sh deploy/macos/uninstall-macos.sh
# 验收 LaunchDaemon 已卸载；此时找不到 job 是预期结果。
sudo launchctl print system/org.sarmg.xscc
```

正式退役前在 Manager 撤销设备并归档必要执行证据。当前没有受支持的自动业务状态 purge 命令。`pkgutil --forget` 只删除安装收据，不会卸载程序，不要用它代替卸载脚本。

## 常见问题与恢复选择

| 现象 | 建议检查/操作 | 验收依据 |
|---|---|---|
| 安装成功却找不到 CLI | Windows 获取注册表安装路径或开新终端；macOS 用绝对路径 | `version` 为预期版本 |
| `connection_unconfirmed` | 保留已保存身份，分别 `doctor --network` 和 `doctor --sunshine` | Manager 在线、Sunshine API 可用、只读任务有结果 |
| Manager TLS/网络失败 | 核对 HTTPS origin、时间、DNS、可信证书和代理/WSS 配置 | 远程检查成功且会话建立 |
| Sunshine 返回 401/403 | 检查 Web UI 凭据，停服后 `credentials update` | `doctor --sunshine` 通过 |
| Sunshine 版本不受支持 | 核对实际版本是否为当前固定适配版本 | 本机接口版本检查通过 |
| Manager 明确拒绝旧设备凭据 | 管理员提供新码，停服后 `pair replace` | 新绑定有效，连接恢复 |
| `pairing_state_incompatible` | 不移动整个目录；用新码 `pair replace` 归档不兼容账户 | 当前身份建立、执行记录保全 |
| `important_state_incompatible` | 停服保全执行记录，排查格式/路径/权限 | 重要记录验证通过后再恢复 |
| 同名服务指向其他程序 | 查看 SCM/systemd/launchd 的程序路径，使用原生包修复 | 服务登记与本产品路径匹配 |

每次配置或凭据写入前停服，成功后启动。任务结果为未确认时先检查实际执行状态，保留任务 ID 和执行记录，不通过清空状态重复可能已执行的操作。

## 安装器、本机 HTTPS 与兼容性补充

适用于 1.0.0。Client 是管理代理；需要先安装 Sunshine 并在其 Web UI 设置用户名、密码。Manager 地址、Manager 配对码、Sunshine 本机地址和 Sunshine 管理凭据是不同的输入。每个平台只发布一个原生安装包，安装器检查平台、架构、权限并注册服务。安装后的逐步配置和配对命令见[完整配置指南](configuration.md)。

### Windows 11 x64

下载并校验 Release 的 Windows MSI。在管理员 PowerShell 中进行普通安装；安装事务只部署程序和服务，不执行交互式 `setup`：

```powershell
msiexec.exe /i .\xscc-1.0.0-windows-x64.msi /norestart
$installRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\xscc').InstallLocation
$client = Join-Path $installRoot 'xscc.exe'
```

MSI 默认登记 Automatic 的 `Xscc` 系统服务。首次未配对时等待设置完成再启动。安装器自动保留配置、凭据和执行记录；兼容性准备会先只读验证执行日志，再把不兼容或损坏的 `identity.json` / `bootstrap.json` 归档为唯一名称（包括可识别的旧格式文档），当前 v1 账户保持不变。安装完成后，在管理员终端显式运行 `& $client setup --interactive` 即可创建当前账户；原有 CLI 会在需要写入受保护状态时请求提权，`--help` 和 `--version` 不触发 UAC。配对失败不会回滚已提交的安装。

交互输入 Manager 管理台生成的配对码、`https://127.0.0.1:47990/`、Sunshine Web UI 用户名和密码。Manager 配对码按普通文本在终端中明文回显，不提供遮罩或隐藏切换；Sunshine 密码仍隐藏输入。默认服务账户 LocalSystem，状态目录 `C:\ProgramData\Xscc`。MSI 会把用户选择的程序目录事务性追加到机器 PATH；新终端可直接运行 `xscc`，卸载会移除该安装器拥有的 PATH 项。

已有版本直接再次运行同一 MSI；原生安装器处理升级、修复、降级检查和服务登记，保留设备身份、凭据和任务记录；MSI 跨版本升级会重新登记 Automatic 服务，可在后续 Setup 中明确选择不开机自启。安装阶段归档不兼容账户后，只需运行普通 `xscc setup` 并输入新的实例授权码；有效 v1 身份仍会直接复用，未完成事务仍会恢复。

交互安装仅提供安装目录等必要选项，没有功能选择页或配置/数据清理开关；完整 Client 始终安装。设置时的开机自启默认 Yes，按 Enter 接受；配对成功后自动启动并检查连接。Windows 使用系统服务，无需用户登录。手工 PowerShell 修复脚本保留已有服务的启动策略和运行状态。

MSI 同版文件修复：`msiexec.exe /i "完整路径\xscc-1.0.0-windows-x64.msi" REINSTALL=ALL REINSTALLMODE=amus /l*v "%TEMP%\xscc-install.log"`（在 cmd 中执行）。安装器忙碌时等待其他安装结束；3010 表示 Windows 需要重启完成替换。使用 `Get-Service Xscc` 和 `xscc service status` 检查服务。

### Ubuntu 24.04 x86_64

```sh
sudo apt install ./xscc_1.0.0_amd64.deb
sudo xscc setup
```

默认配置与状态位于 `/var/lib/xscc`，运行账户 `xscc`。首次安装默认通过系统级 `systemctl enable xscc.service` 设置开机自启，unit 安装于 systemd 系统目录，启动目标为 `multi-user.target`。未完成配对时保持未运行；设置仅询问开机自启，默认 Yes，按 Enter 接受，随后自动启动并验证连接。已有安装升级/修复保留原来的 systemd 启停策略。日志：`sudo journalctl -u xscc.service -n 100 --no-pager`。

同版损坏执行 `sudo apt install --reinstall ./xscc_1.0.0_amd64.deb`。若状态来自 v1，先创建新授权码，再运行 `sudo xscc pair replace --interactive`；Client 会归档旧账户文档，执行日志异常时则保留所有原件并明确报错。若安装器没有交互终端，稍后手动运行该命令。

手工归档只包含可执行文件、文档和校验元数据，不包含 Shell/Python 安装包装脚本；生产安装和覆盖请使用 DEB。解压的可执行文件可用于临时诊断，但不替代原生服务安装器。

### macOS Apple Silicon

只提供 Apple Silicon 原生 PKG，不提供 Intel 版本：

```sh
sudo installer -pkg ./xscc-1.0.0-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/xscc setup
```

状态路径 `/Library/Application Support/xscc`，系统服务账户 `_xscc`；LaunchDaemon `org.sarmg.xscc` 无需用户登录即可运行。诊断命令 `sudo launchctl print system/org.sarmg.xscc`，日志 `/var/log/xscc.log`。

再次运行原生 PKG 即可覆盖旧程序，或在保留状态的卸载后重装。安装器保留身份和执行记录；随后运行 `/usr/local/bin/xscc setup`。发行文件未签名、未公证，使用系统提供的本地批准入口允许已校验的程序运行，不要全局关闭系统安全检查。

本次名称复查统一了 macOS 账户 `_xscc`。覆盖安装和修复只接受该账户对应的实际 UID、受保护状态和日志属主；如果此前安装使用了其他名称的服务账户，安装器会保留旧文件并拒绝接管。先停止服务，用原兼容版本核对尚未确认的任务并导出配置，在 xscs 撤销旧绑定；将旧状态和日志作为受保护归档保留后，再按本章进行全新安装和配对。不要重命名系统账户、复用旧 UID 或通过递归 `chown` 绕过检查。

### Sunshine 服务控制

Manager 发起的 Sunshine 服务控制使用固定的本机服务：Windows 为 `SunshineService`，Ubuntu 为系统级 `sunshine.service`；macOS 不上报此能力。Windows 上的启动、停止在服务已处于目标状态时直接确认，重启已停止的服务会启动它。服务管理器单次命令最多等待 20 秒；支持的平台都要读回运行或停止状态，状态未达到目标时操作结果不能视为成功。

### 本机 Sunshine HTTPS 与认证边界

Client 只接受 `https://127.0.0.1:<port>/` 或等价的 IPv6 回环 IP 字面量。主机名、非回环地址、HTTP、URL 用户信息、查询、片段和额外路径都会在连接前被拒绝；请求不使用系统代理，也不跟随重定向。

Sunshine 默认使用本机自签名证书。由于连接被限制在内核回环接口，本版本不校验 Sunshine 证书链、名称、指纹或固定值，也不读取和保存 `cacert.pem`。TLS 仍用于加密连接，每个 API 请求均携带受保护状态中的 Sunshine Basic Auth 凭据；401/403 会作为凭据拒绝处理。Manager 是独立的远程安全边界，始终使用标准 WebPKI 身份校验和设备长期凭据认证。Manager 在数据库确认设备凭据无效时，WebSocket 握手返回带有 `X-Xcss-Error-Code: unauthorized` 的 401，Client 停止重连并等待管理员处理凭据；不带该标识的 401（包括代理丢失认证头、旧 Manager 或代理自身的响应）、入口校验 403 或服务暂不可用时 Client 按退避策略重连。

公开的首次配对入口使用受保护的 stdin JSON：

```json
{"server":"https://manager.example.org/","authorization_code":"REPLACE_WITH_INSTANCE_CODE","sunshine_endpoint":"https://127.0.0.1:47990/","sunshine_username":"REPLACE_LOCALLY","sunshine_password":"REPLACE_LOCALLY"}
```

此 JSON 只可交给 `xscc setup --input-stdin --non-interactive` 的 stdin。当前格式拒绝 `sunshine_certificate`、`sunshine_certificate_path`、`sunshine_ca_pem` 和 `use_system_trust` 等旧字段。秘密不得放进命令参数、部署日志或版本库。`credentials update` 只接受新的 Sunshine 用户名和密码。

### 已有配置、升级与故障处理

`setup` 按配置、配对、服务注册、启动策略、运行状态和连接顺序执行后置验证。交互终端会逐步显示 `verified`；JSON 失败响应中的 `error.step` 指明失败关卡，`error.code` 和 `error.message` 给出稳定原因，操作系统服务命令失败时 `error.detail` 保留经过控制字符清理和长度限制的原始诊断。服务必须真实运行，连接验证固定执行；未运行或无法确认连接时返回对应失败。设置写入完成与连接确认是不同层次；如果返回 `connection_unconfirmed`，保留现有身份并分别运行 `doctor --network` 和 `doctor --sunshine`，不能把它视为 Manager 与 Sunshine 均已连接，也不要删除状态重新配对。

当前 v1 有效身份无需重复初始化或配对。再次运行 `setup` 会复用身份，待处理事务会调用 `pair resume`；`config show --format json` 查看脱敏配置与修订，候选配置只支持 `sunshine_endpoint`；用 `config validate/diff/apply --file <绝对路径>`，提交还需要 `--expected-revision <当前修订>`，写入前停止服务。配对后直接启用当前平台支持的 Sunshine 专用管理能力。

若 `status` 返回 `pairing_state_incompatible`，不要重命名整个状态目录。先在 Manager 创建新实例授权码，再运行 `pair replace --interactive`。Client 会先只读验证执行日志；验证通过后将旧 `identity.json` 归档为唯一的 `identity.incompatible-*.json`，验证失败则返回 `important_state_incompatible`，且配对资料与执行日志均保持原样。

`--config` 选择整个状态目录（`--state` 是兼容别名）；系统服务操作应使用上面的默认目录。配对响应丢失使用 `status`、`pair status` 或 `pair resume`；不要用清空状态来重试。强制文件覆盖不会清除设备身份、凭据或执行记录。若检测到指向其他程序的同名服务、符号链接或不可信目录，安装会给出错误并保留数据；先检查对应路径，避免盲目删除。

### Windows 后台诊断

SCM 在私有状态校验后创建 `%ProgramData%/Xscc/logs`，使用共享 typed sink 写入 `xscc.jsonl`。最多保留活动文件和四份归档，每份 8 MiB，总上限 40 MiB。服务账户首次创建此目录；管理员查询不会先替服务建立日志目录。ACL 拒绝普通用户，已有不安全对象不修复。服务启动失败且日志 sink 尚不可用时，可同时查看 Windows SCM 的服务退出代码。

```powershell
xscc logs --tail 100 --format json
xscc logs --since 2026-10-07T00:00:00Z --level warn --format json
xscc logs --follow --format ndjson --timeout 60s
```

可用 `--instance-id`、`--event`、`--request-id` 和 `--task-id` 精确筛选。后台整体启动事件具有服务 scope，只有确实属于某个已知实例的事件才带 instance_id。日志源损坏、超限或 follow 游标已从保留窗口移除时明确失败，避免把丢失记录显示为空成功。
