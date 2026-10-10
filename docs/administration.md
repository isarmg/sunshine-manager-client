# 维护 xscc

以下命令在客户端主机执行。Linux 使用 sudo；macOS 用 `sudo /usr/local/bin/xscc`；Windows 管理员 PowerShell 中直接使用 `xscc`。

## 服务启停与自启

| 目的 | 命令 |
|---|---|
| 查看登记、运行和自启 | `xscc service status --format json` |
| 立即启动、停止、重启 | `xscc service start`、`stop`、`restart`，选择一项 |
| 设置开机自启 | `xscc service enable` |
| 取消开机自启 | `xscc service disable` |
| 取消自启并立即停止 | `xscc service disable --now` |

这些命令管理 xscc 代理；Sunshine 自己的服务状态独立。Windows 管理目标为 SunshineService，Ubuntu 为系统级 sunshine.service；macOS 在本机管理 Sunshine 服务。

## 更新实例授权码

管理员更换同一实例的授权码后，停止客户端，输入新码并检查保存结果：

```sh
sudo xscc service stop
sudo xscc pair replace --interactive
sudo xscc pair status --format json
sudo xscc service start
sudo xscc status --check --format json
```

本机安装身份与任务记录保留。若只是配对请求响应丢失，使用 `pair resume` 继续现有事务。

## 更新 Sunshine 用户名和密码

Sunshine Web UI 中的凭据改变后，在 Client 主机执行：

```sh
sudo xscc service stop
sudo xscc credentials update --interactive
sudo xscc doctor --sunshine --format json
sudo xscc service start
```

此操作保留管理端绑定。回环 Sunshine HTTPS 使用加密连接与 Basic Authentication；为适配自签名证书，客户端不检查该回环连接的证书身份。远程管理端仍验证系统信任链和名称。

## 平台日志与路径

- Linux：`sudo journalctl -u xscc.service -n 100 --no-pager`
- macOS：`sudo tail -n 100 /var/log/xscc.log`；安装日志在 `/var/log/install.log`
- Windows：`xscc logs --tail 100 --format json`，早期启动失败查看 SCM 服务退出码和系统事件

Windows 持久 JSON 日志默认每份 8 MiB，活动文件加四份归档共 40 MiB。可用 `logs --follow --format ndjson --timeout 60s` 有界跟踪。日志读到损坏或已过保留窗口会明确报错。

默认状态目录：Windows `C:\ProgramData\Xscc`，Linux `/var/lib/xscc`，macOS `/Library/Application Support/xscc`。`--config` 指整个目录；系统服务使用默认位置。任务执行记录作为去重和结果依据保留。

## 修复或卸载当前安装

同版原生包可修复程序与服务文件，保留业务状态。Windows MSI 完成后检查退出码，3010 表示需重启；Linux DEB 可用 `sudo apt install --reinstall ./xscc_1.0.0_amd64.deb`；macOS 重新执行已校验 PKG 的 installer 命令。随后检查版本、服务和业务连接。

普通卸载步骤见[安装指南](platform-setup.md)。客户端没有自动清除业务状态的命令。
