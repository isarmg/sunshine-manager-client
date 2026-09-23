# Sunshine Client 0.2.10 当前兼容边界

Client 支持 Windows x64、Linux x64 和 macOS Apple Silicon。部署时核对安装包版本、源码身份、Manager 版本及本机 Sunshine 版本。

| 维度 | 当前契约 |
| --- | --- |
| Client 程序 | 0.2.10 |
| Manager | 0.11.11 |
| Manager 协议 | sunshine-management/2；sunshine-client-protocol 0.2.0，固定提交 3501d1eebd5ad16abb52c9901cbef745a1d53afb |
| Sunshine | v2026.914.233613 |
| Client Foundation | 0.9.16，固定提交 9bf6eec21f42188c73105eef2f369bf47c8b8f86 |
| 设备账户 | 当前 v2 授权与配对状态 |
| IPC | Foundation GetStatus/1，校验进程世代、安装身份与配置修订 |

安装器部署程序与服务，并按用户选择保留本机设置、凭据和执行日志。v1 配对资料会被识别为 pairing_state_incompatible；管理员创建新授权码后使用 pair replace --interactive，Client 先检查执行日志，再归档不兼容账户文档。执行日志不兼容或不可读时返回 important_state_incompatible 并保留原件。

Windows SQLite 与 Unix 文件状态不能跨平台复制。产品仓不提供自动历史转换或降级；具体安装与修复步骤见平台安装指南。
