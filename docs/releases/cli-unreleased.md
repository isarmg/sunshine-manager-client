 > 这是 0.1.0-rc.5 的历史验收记录。当前覆盖安装行为和操作步骤见 [0.1.0-rc.9](0.1.0-rc.9.md) 与 [平台指南](../platform-setup.md)。

# 纯命令行改造（v0.1.0-rc.5 预发布）

当前支持 Windows x64、Linux x64、macOS Apple Silicon。Intel macOS 已从适配、CI 和发行范围移除；下文早期验证记录仅供追溯。

公开入口为产品 CLI，长期运行由 SCM、systemd 或 launchd 管理。删除托盘、网页控制服务和登录自启入口；服务端集中管理网页保留。没有版本号或持久状态格式的暗中迁移。

管理员先停 Client 服务，再进行配对、配置提交或凭据更新。外层维护锁先于运行实例/状态事务锁获取，冲突有界失败；查询不创建锁和缺失状态。现有身份、凭据、Host 队列和 Sunshine 执行记录继续使用原格式。

## 公共用法

- `version` / `--version`；机器输出增加 `--format json`。
- `config init`，`config show`，`config validate --file /absolute/candidate.json`。
- `config diff --file /absolute/candidate.json`。
- `config apply --file /absolute/candidate.json --expected-revision REVISION`。
- `pair --interactive` 或 `pair --input-stdin --non-interactive --format json`。
- `pair status`，`pair resume`。恢复必须找到已有事务。
- `status`，`status --watch --format ndjson --timeout 5m`，`status --check`。
- `doctor` 只读；`doctor --network` 显式访问公共健康端点，报告当前 CLI 账户的信任环境。
- `service status|start|stop|restart|enable|disable`；`enable/disable --now` 同时操作当前运行状态。
- Linux：`logs --tail 100`、`logs --since '2026-09-07'`、`logs --follow --format ndjson`。

`--output` 是 `--format` 的兼容别名。默认不输出终端控制序列。输入 JSON 最多 64 KiB，拒绝未知字段；禁止把授权码、密码或长期凭据作为参数。交互秘密输入不回显。JSON 有固定 schema 和错误代码；超时后先查询已有事务，不重新生成身份。

配置修订与生效修订分别展示。Linux/macOS 的只读 Unix socket 使用受保护目录、对端身份、固定请求、版本与大小限制，绑定进程世代及身份；旧绑定摘要不能作为新绑定事实。无法获得运行事实时明确返回 unavailable/unknown。`status --check` 不会把未证实的健康状态当作成功。

## 原生验证与发行边界

[四个平台的原生服务及安装包 CI 已通过](https://github.com/isarmg/sunshine-manager-client/actions/runs/34193661720)，源码提交 `6127a9d55c0125cca1bc0bc3c32344466a8b64ec`：Windows、Linux、macOS Intel 与 Apple Silicon。验收使用一次性 runner 的合成离线身份，覆盖安装后停止、显式启动与启动策略、真实只读 IPC、运行中维护冲突、管理员凭据更新、服务账户访问、停止后不错误重启，以及卸载保留身份和执行日志。

Mac 归档已加入源提交、清单、校验和及实际可执行文件身份的独立验证。Windows 使用 MSVC/MSI，Linux 使用 Ubuntu 24.04 的 DEB；本次 GitHub Release 标记为预发布，安装产物未签名，未公证。

协议固定为 Server 的真实提交 `d4b98b06a00d185bd845a4bd3e3df8c939807864`，已删除临时 vendor；[Server 完整 CI](https://github.com/isarmg/sunshine-manager-server/actions/runs/34192112410) 已通过。生产环境仍须先部署匹配的 Server，再连接 macOS Client。

版本与升级/回退边界见 [兼容矩阵](cli-compatibility.md)。不存在自动历史迁移、跨平台状态复制或自动降级能力；不能通过删除日志或重新配对绕过安装器的覆盖拒绝。

Windows 11 上已用本次原生 CLI 对隔离的真实 Sunshine 实例执行 `doctor --sunshine`：未信任证书时退出码为 6；在当前用户证书存储临时信任测试 CA 后，认证读取成功。前后哈希证明 Client SQLite 状态与 Sunshine 配置未变；结束后已核实临时 CA 和隔离进程均被清理。此项未操作已有生产 Sunshine 服务，只证明当前用户的 TLS 信任环境，不代表后台服务账户的信任环境，也未验证真实受控写入与重启闭环。

用户确认目前只有 Windows Sunshine 主机，暂无 Linux/macOS Sunshine 实机。CI 的离线服务验收不等于这两平台的真实 Sunshine 联调，也不等于物理重启后无人登录验收。
