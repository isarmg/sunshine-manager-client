# Sunshine Client 0.2.7 兼容边界

当前正式版本为 `0.2.7`，支持 Windows x64、Linux x64 和 macOS Apple Silicon。部署时同时核对版本、源码
身份、Manager 版本和 Sunshine 固定版本。

| 维度 | 当前契约 |
|---|---|
| Manager 协议 | 仅 `sunshine-management/2`；不协商 v1 |
| Sunshine | 仅 `v2026.914.233613` |
| Client Foundation | `0.9.14`，固定提交 `8b8ea8517a3cf68e566f8230e87bae9ab40b4106` |
| Bootstrap | 当前严格字段；旧 `restart_allowed`、应用权限和服务模式字段会被拒绝 |
| 能力 | 配对后直接启用 Sunshine 专用能力；服务控制按平台固定适配器声明 |
| IPC | Foundation `GetStatus/1`，进程世代、安装身份和配置修订校验 |

0.2.7 不把旧 Bootstrap、旧协议身份或旧任务当作当前输入，也不猜测式降级。Client 会把 v1 配对资料识别为
`pairing_state_incompatible`；创建新授权码后运行 `pair replace --interactive`，只归档不兼容的账户文档。替换前会
完整检查执行日志，日志不兼容或不可读时返回 `important_state_incompatible` 并保留账户资料和日志原件。

Windows SQLite 与 Unix 文件状态不能跨平台复制。产品仓不提供猜测式转换；只有 `sarmg-upgrade` 明确列入
版本矩阵的转换才受支持。卸载、重新安装或手工修改版本号均不构成状态迁移。
