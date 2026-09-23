# Sunshine Client 文档

本文档集描述当前 `0.2.10` Client。命令与持久状态以当前源码、`sunshine-client --help` 和
`sunshine-client version --format json` 为准；`releases/` 按版本保存发行记录，当前操作以本目录指南为准。

| 文档 | 内容 |
|---|---|
| [../README.md](../README.md) | GitHub 首页简介、最短配置路径和开发验证 |
| [configuration.md](configuration.md) | 本机设置、Manager 配对、凭据更新、授权码轮换和诊断的完整命令 |
| [platform-setup.md](platform-setup.md) | Windows、Ubuntu 与 macOS 安装、升级和平台边界 |
| [releases/cli-compatibility.md](releases/cli-compatibility.md) | 当前协议、Foundation、平台、状态格式和升级边界 |
| [releases/0.2.10.md](releases/0.2.10.md) | 当前版本发行说明 |
| [releases/](releases/) | 按版本保存的发行记录 |

当前 Client 是 CLI 与系统服务，不含托盘或本地 Web。它主动连接 Manager，只管理本机 Sunshine；不会代理
Sunshine–Moonlight 媒体流。旧 v1 账户状态会被准确识别，并通过显式重新配对安全归档；执行日志不会被账户恢复流程删除或覆盖。
