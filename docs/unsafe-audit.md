# xscc 原生接口审查

当前依赖来源与精确版本以 Cargo.toml 和 Cargo.lock 为准。本页说明保留的原生接口及其安全条件，便于修改对应模块时复核。

| unsafe 所属代码 | 必要边界与条件 | 处理 |
|---|---|---|
| Windows 文件/ACL 层 | 原生令牌、SID、安全描述符、文件元数据和受保护 DACL；分配、对齐、句柄唯一所有权及描述符有效期均需保留。 | 重复的打开器、令牌、SID 和 ACL 验证已删除；完全消费 xcsc 拥有的私有文件守卫。SQLx 先显式关闭，再释放守卫。 |
| `process_identity` | Windows 原生进程快照、创建时间与 TCP 监听者证据，用于区分真正的 Sunshine 世代；std::process 没有该接口。 | RAII 关闭成功句柄；初始化 SDK 结构大小；表缓冲区最多 1 MiB，确认返回长度和每行范围后读取，任何不确定返回 `unknown`。 |
| `elapsed_clock` | Windows GetTickCount64 与 Apple mach_continuous_time 计入休眠时间；std::time::Instant 无法保证相同的平台语义。 | 无参数时钟调用；时间基准输出是已初始化、正确 C 布局的两个 u32；使用 u128 运算并检查结果范围。 |

## 复核与测试

`unsafe_op_in_unsafe_fn = deny` 要求每个原始操作使用显式 unsafe 块。检查句柄和内存所有权、长度与对齐、异常返回、生命周期和释放；平台 SDK 更新时核对结构及调用 ABI。

运行[开发指南](development.md)的测试及对应原生平台 CI。协议模拟、交叉编译、安装器和实机硬件分别记录验证结果。通用私有文件、锁和日志由 xcsc 提供，产品保留实际业务的状态与身份选择。
