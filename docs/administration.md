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

这些命令管理 xscc 代理；Sunshine 自己的服务状态独立。各平台管理目标见对应平台指南。

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

## 本机日志、修复和卸载

按所用平台查看日志路径、同版包修复及卸载步骤：

- [Linux](platforms/linux.md#日志与本机状态)
- [Windows](platforms/windows.md#日志与本机状态)
- [macOS](platforms/macos.md#日志与本机状态)

任务执行记录是去重和恢复结果的依据；普通卸载会保留这些记录，客户端没有自动清除业务状态的命令。
