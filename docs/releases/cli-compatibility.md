# Sunshine Client 0.2.0 兼容边界

当前正式版本为 `0.2.0`，支持 Windows x64、Linux x64 和 macOS Apple Silicon。部署时同时核对版本、源码
身份、Manager 版本和 Sunshine 固定版本。

| 维度 | 当前契约 |
|---|---|
| Manager 协议 | 仅 `sunshine-management/2`；不协商 v1 |
| Sunshine | 仅 `v2026.914.233613` |
| Client Foundation | `0.9.1`，固定提交 `2fa783a1eb7aefde8e54e328f91d67aa9e1eb890` |
| Bootstrap | 当前严格字段；旧 `restart_allowed`、应用权限和服务模式字段会被拒绝 |
| 能力 | 配对后直接启用 Sunshine 专用能力；服务控制按平台固定适配器声明 |
| IPC | Foundation `GetStatus/1`，进程世代、安装身份和配置修订校验 |

0.2.0 不读取旧 Bootstrap、旧协议身份或旧任务作为兼容输入，也不自动降级。部署 0.2.0 前停止旧服务、
保全旧状态用于审计，在 Manager 0.11.0 创建/轮换授权码，并用当前受保护输入重新运行 `setup`。不要删除旧日志
后假定副作用没有发生。

Windows SQLite 与 Unix 文件状态不能跨平台复制。产品仓不提供猜测式转换；只有 `sarmg-upgrade` 明确列入
版本矩阵的转换才受支持。卸载、重新安装或手工修改版本号均不构成状态迁移。
