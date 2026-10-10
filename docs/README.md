# xscc 文档

本文档集描述当前 `1.0.0` Client。命令与持久状态以当前源码、`xscc --help` 和
`xscc version --format json` 为准；`releases/` 按版本保存发行记录，当前操作以本目录指南为准。

| 文档 | 内容 |
|---|---|
| [../README.md](../README.md) | GitHub 首页简介、最短配置路径和开发验证 |
| [configuration.md](configuration.md) | 本机设置、Manager 配对、凭据更新、授权码轮换和诊断的完整命令 |
| [platform-setup.md](platform-setup.md) | Windows、Ubuntu、macOS 安装、配对/更换、凭据更新、服务查看/启停、诊断、升级与卸载，含命令解释 |
| [releases/1.0.0.md](releases/1.0.0.md) | 当前协议、xcsc、平台、状态格式和升级边界 |
| [unsafe-audit.md](unsafe-audit.md) | 依赖选择、unsafe 边界与验证 |
| [releases/](releases/) | 按版本保存的发行记录 |

当前 Client 是 CLI 与系统服务，不含托盘或本地 Web。它主动连接 Manager，只管理本机 Sunshine；不会代理
Sunshine–Moonlight 媒体流。旧 v1 账户状态会被准确识别，并通过显式重新配对安全归档；执行日志不会被账户恢复流程删除或覆盖。

CLI 参数、输出与兼容性约定见 [CLI 兼容性](cli-compatibility.md)。

公共支撑的职责、单体依赖、平台边界与验证方法见[公共支撑说明](common-support.md)。
