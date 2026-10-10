# 排查 xscc 问题

安装器、命令路径和系统日志问题按平台查看：[Linux](platforms/linux.md#排查本机问题)、[Windows](platforms/windows.md#排查本机问题)、[macOS](platforms/macos.md#排查本机问题)。

以下排查各桌面平台共用。先运行 `version --format json`、`service status --format json` 和 `logs --tail 100`。按下表继续；维护命令使用管理员权限。

| 现象 | 检查 | 下一步与完成结果 |
|---|---|---|
| 服务运行但连接未确认 | `status --format json` 和日志中的错误 code | 分别定位网络、认证和业务检查 |
| DNS、TLS 或超时 | `doctor --network --format json` | 核对根地址、DNS、主机时间、系统信任链和证书名称 |
| 配对响应丢失 | `pair status --format json` | `pair resume` 继续同一事务 |
| 授权码轮换后认证失败 | 确认同一实例的新授权码 | 按[维护指南](administration.md)重新绑定，再确认业务连接 |
| 配置修改未生效 | `config show --format json` | 停服后 validate/diff/apply，再启动并检查新修订 |
| `important_state_incompatible` | 查看错误指向的执行记录、权限与日志 | 停服保留数据，核对安装版本与状态完整性；记录可读后再恢复 |
| Sunshine 401/403 | `doctor --sunshine --format json` | 在本机更新 Sunshine 凭据，验证成功后启动 |
| Sunshine 不可用或版本不支持 | 同一检查与 Sunshine 本机 Web UI | 确认受支持版本、回环 HTTPS 地址与服务状态 |
| 任务 unknown | `tasks show OPERATION_ID --format json` 与实际设备状态 | 保留记录并人工核对；先确定效果，再决定后续动作 |
| 管理端 HTTPS 正常但会话掉线 | WSS 代理及 `/xscc/v1/connect` | 保留 Upgrade，核对代理超时与管理端日志 |

`doctor --network` 只检查远端公开入口，本机 Sunshine 检查和管理任务往返分别验证另外两段连接。远端证书应由实际服务账户的系统信任库信任。

`setup` 超时或 Ctrl+C 可能发生在身份已经保存之后。先读配对和日志结果，再继续原事务，避免丢弃已有状态。提交问题时附软件版本、平台、脱敏 code 和所做检查，保留凭据及原始业务数据在本机。
