# xscc 当前契约与联调边界

此表描述 1.0.0 的实现契约与受控输入。实际安装包和最终 Source 的原生验证见对应 Release；接口与持久状态身份不由软件版本推导。

| 维度 | 当前契约 |
| --- | --- |
| Client 程序 | 1.0.0 |
| Manager 通信 | sunshine-management/1；WSS subprotocol sunshine-management.v1；/xscc/v1/ |
| 业务任务身份 | TASK_PROTOCOL = sunshine-management/1；现有任务结构、权限、绑定与指纹字节不变 |
| 产品协议输入 | xscs-protocol 1.0.0，官方 Git revision f7943abee4bb74eb6b2a86687ca1ce28028070d9 和精确 crate 版本 |
| Sunshine | v2026.914.233613 |
| Client Foundation | 0.10.5 / ab53bb6157117169b6d03d0a61497faa9ca718bd，真实已提交源码 |
| 设备账户 | 当前 v1 授权与配对状态 |
| IPC | Foundation GetStatus/1，校验进程世代、安装身份与配置修订 |

通信契约新增必填 configuration_overwrite 与 pending_pairing_listing。客户端按真实实现声明；Server 按能力及权限验证，不根据 client_version 推测。只接受当前通信身份，没有旧接口转发或自动回退。Task 的语义没有改变，因此使用独立的既有任务身份；升级不会重写、遗忘或重做既有副作用记录。

产品协议已替换为官方完整 Git revision 与精确 crate 版本，Cargo.lock 固定同一来源。package gate 明确拒绝 path、浮动 revision、其他源或不一致的 lock；最终原生包仍需针对这份 Source 验证。

安装器默认保留设置、凭据和执行事实；Windows SQLite 与 Unix 文件状态不能跨平台复制。执行日志异常时保留原件并返回明确错误；不能通过重新配对或清除 journal 绕过未确认副作用。

当前软件基线为 1.0.0，配套安装与诊断步骤见[分平台部署指南](platform-setup.md)。
