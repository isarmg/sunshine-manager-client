# xscc 命令参考

使用 `xscc --help` 查看当前可用命令，`xscc version --format json` 查看软件与协议身份。Linux 维护命令使用 sudo，Windows 使用管理员 PowerShell，macOS 使用 `sudo /usr/local/bin/xscc`。

## 按目的选择命令



| 命令 | 用途与影响 |
|---|---|
| `version --format json` | 只读确认软件、平台及当前协议/账户版本 |
| `config init [--interactive]` | 首次初始化本机 Sunshine 接口设置，不用于清理旧身份 |
| `config show --format json` | 查看脱敏设置和 `stored_revision` |
| `config edit` | 在编辑器中修改候选 Sunshine endpoint，经校验和修订检查后提交；先停服 |
| `config validate --file PATH` | 仅校验候选，不应用它 |
| `config diff --file PATH` | 比较候选与当前设置，供审阅 |
| `config apply --file PATH --expected-revision REVISION` | 检查当前修订并提交候选，避免并发覆盖；先停服 |
| `setup` | 完整配对/复用、服务策略、启动和连接验证，适合首次部署 |
| `pair` / `pair status` / `pair resume` | 分别执行单独配对、查看事务、恢复已开始事务 |
| `pair replace` | 用新管理端授权替换绑定，保留本机安装身份和执行记录 |
| `credentials update` | 更新本机 Sunshine 用户名/密码，保留管理端绑定；先停服 |
| `status --check` | 查看客户端业务与连接检查，不能代替只读任务往返验收 |
| `doctor --network` | 检查远程管理端入口，不能证明 Sunshine 本机认证可用 |
| `doctor --sunshine` | 真实访问本机 Sunshine API，检查版本与凭据 |
| `tasks list` / `tasks show OPERATION_ID` | 查看任务列表/单项持久执行记录，不重新执行任务 |
| `logs --tail 100` | 读取最近日志，不改变服务运行状态 |

`--config` 指向整个状态目录，`--file` 指向实际候选文件，`REVISION` 从最新 `config show` 复制。`--interactive` 通过受保护终端输入，`--input-stdin` 从受保护 stdin 读严格 JSON；`--non-interactive` 禁止额外提示，`--timeout` 限定等待，`--format json` 提供结构化结果。系统服务操作必须使用默认状态目录。



## 服务操作

| 命令 | 效果 |
|---|---|
| `service status --format json` | 查看服务登记、运行状态与自启策略 |
| `service start` / `service stop` / `service restart` | 立即启停，不改变自启策略 |
| `service enable` / `service disable` | 设置开机策略，不改变当前进程 |
| `service disable --now` | 取消自启并停止服务 |

`status --check` 同时通过退出码表达检查结果，`--format json` 提供结构化内容；自动化应检查退出码和稳定错误 code。日志字段与平台路径见[维护指南](administration.md)。
