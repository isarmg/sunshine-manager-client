# xscc 文档

本文档集描述当前 `1.0.0` 客户端。命令与持久状态以当前源码、`xscc --help` 和
`xscc version --format json` 为准；`releases/` 按版本保存发行记录，当前操作以本目录指南为准。

| 文档 | 内容 |
|---|---|
| [../README.md](../README.md) | 项目简介、功能、平台、快速部署和编译部署 |
| [configuration.md](configuration.md) | 本机设置、管理端配对、凭据更新、授权码轮换和诊断的完整命令 |
| [platform-setup.md](platform-setup.md) | Windows、Ubuntu、macOS 安装、配对/更换、凭据更新、服务查看/启停、诊断、升级与卸载，含命令解释 |
| [releases/1.0.0.md](releases/1.0.0.md) | 当前协议、xcsc、平台、状态格式和升级边界 |
| [unsafe-audit.md](unsafe-audit.md) | 依赖选择、unsafe 边界与验证 |
| [releases/](releases/) | 按版本保存的发行记录 |

当前客户端是 CLI 与系统服务，不含托盘或本地网页。它主动连接管理端，只管理本机 Sunshine；不会代理
Sunshine–Moonlight 媒体流。有效的当前 v1 账户状态可继续使用；未知或损坏的账户状态先保全，再按部署指南明确处理。执行日志不会被账户恢复流程删除或覆盖。

CLI 参数、输出与兼容性约定见 [CLI 兼容性](cli-compatibility.md)。

公共支撑的职责、单体依赖、平台边界与验证方法见[公共支撑说明](common-support.md)。

## 开发与通信

- [开发与验证](development.md)：工具链、测试、原生打包、仓库结构与凭据边界
- [管理端通信与执行恢复](https://github.com/isarmg/xscs/blob/main/docs/communication-reliability.md)：任务持久化、确认和 WSS 中断处理
