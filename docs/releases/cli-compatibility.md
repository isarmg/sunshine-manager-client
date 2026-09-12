# CLI 预发布兼容矩阵

本次预发布版本为 `0.1.0-rc.8`，支持 Windows x64、Linux x64 和 macOS Apple Silicon；Intel macOS 不再适配或发行。部署时同时核对版本与源码身份。

| 维度 | 契约 |
| --- | --- |
| CLI JSON | `schema_version = 1` |
| 配置格式 | `sunshine-bootstrap-v1`，旧 bootstrap 严格字段保持 |
| 身份与日志格式 | `sunshine-identity-journal-v1`；Unix 私有文件、Windows SQLite 后端保持 |
| IPC | `GetStatus/1`，进程世代、安装身份和配置修订校验 |
| 协议依赖 | Server 提交 `d4b98b06a00d185bd845a4bd3e3df8c939807864`，增加真实 macOS 平台值 |

| 来源 → 目标 | 数据兼容性 | 安装与回退边界 |
| --- | --- | --- |
| Windows/Linux 基线 `81b3a51e5668ea6734e7bad53a1db5ab39cf3012` → CLI 验证提交 | 同平台身份、凭据、执行记录后端保持；凭据更新不重注册、不清日志 | 现有安装器的覆盖拒绝仍有效，不能将直接替换二进制当作已支持的包升级 |
| 全新 macOS → CLI 验证提交 | 新平台，使用 Unix 后端；上报真实 Apple Silicon 架构 | 必须先部署能识别新平台的 Server。通过 Apple Silicon 原生 CI 后发行 |
| CLI 验证提交 → 原基线 | 未引入数据格式转换，但旧版本带有托盘启动行为 | 不支持自动降级。停服务并保留全部状态；不得恢复陈旧日志而重放副作用 |
| Windows SQLite ↔ Unix 文件、其他历史格式 | 不支持跨平台复制或猜测转换 | 需要 `sarmg-upgrade` 的独立、精确适配器；目前没有此迁移边 |

当前 UI 删除不需要重写身份或执行日志，因此没有在 Client 中加入自动迁移。`sarmg-upgrade` 的 Server 数据恢复命令不能代替 Client 迁移。手工修改版本、删除执行记录或重新配对均不是升级方案。

`init --bootstrap` 保留为复用同一导入实现的兼容入口。Windows 删除托盘后默认手动启动；管理员明确选择 `service start` 或 `service enable --now`。配对不改变开机策略，停止 Client 不停止 Sunshine 本体。卸载保留身份与日志；当前手动脚本遇到保留的旧目录或账户会拒绝覆盖，不能把卸载后重新运行该脚本描述成已支持的恢复流程。
