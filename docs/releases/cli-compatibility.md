# Sunshine Client 0.3.2 当前兼容边界

Client 支持 Windows x64、Linux x64 和 macOS Apple Silicon。部署时核对安装包版本、源码身份、Manager 版本及本机 Sunshine 版本。

| 维度 | 当前契约 |
| --- | --- |
| Client 程序 | 0.3.2 |
| Manager | 0.12.1 |
| Manager 协议 | sunshine-management/3；sunshine-client-protocol 0.4.0，固定提交 2998876d091e2c1666de36b86f7417f80f04fded |
| Sunshine | v2026.914.233613 |
| Client Foundation | 0.9.16，固定提交 9bf6eec21f42188c73105eef2f369bf47c8b8f86 |
| 设备账户 | 当前 v2 授权与配对状态 |
| IPC | Foundation GetStatus/1，校验进程世代、安装身份与配置修订 |

安装器部署完整 Client 与系统服务，默认保留本机设置、凭据和执行日志，没有功能选择或数据清理开关。Linux 首次安装默认启用 systemd 开机启动，Windows 注册 Automatic 系统服务。Setup 的开机自启默认 Yes，完成配对后自动启动并验证连接。v1 配对资料会被识别为 pairing_state_incompatible；管理员创建新授权码后使用 pair replace --interactive，Client 先检查执行日志，再归档不兼容账户文档。执行日志不兼容或不可读时返回 important_state_incompatible 并保留原件。

Windows SQLite 与 Unix 文件状态不能跨平台复制。产品仓不提供自动历史转换或降级；具体安装与修复步骤见平台安装指南。
