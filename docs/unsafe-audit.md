# 依赖和 unsafe 审查

使用 Rust 1.99.0、SQLx 0.9.0 SQLite bundled driver、rand 0.10、base64 0.23、SHA-2 0.11 和 tokio-tungstenite 0.30。reqwest 0.13、Tokio 1、windows-service 0.8.1 和 Windows SDK bindings 保留适用稳定版本。normal/build/dev/Cargo.lock 不含 rusqlite。

SQLx 的直接 SQLite worker 支持同步调用而不要求产品创建 Tokio runtime；本产品保留一个连接及真实 transaction。数据库 schema 和 FULL durability 是产品状态合同，不由软件发行号派生。[SQLx 官方 runtime 说明](https://docs.rs/sqlx/0.9.0/sqlx/#runtime-support)。查询形状固定，数据通过参数绑定；名称和值的预算在查询和读取前执行。

| unsafe 所属代码 | 必要边界与条件 | 处理 |
|---|---|---|
| Windows 文件/ACL 层 | 原生 token、SID、security descriptor、文件 metadata 和 protected DACL；分配、对齐、句柄唯一所有权及 live descriptor 均需保留。 | 重复 opener、token、SID 和 ACL 验证已删除；完全消费 Foundation owned private-file guard。SQLx 先显式关闭，再释放 guard。 |
| process_identity | Windows 原生进程快照、创建时间与 TCP 监听者证据，用于区分真正的 Sunshine 世代；std::process 没有该接口。 | RAII 关闭成功句柄；初始化 SDK struct size；表缓冲区最多 1 MiB，确认返回长度和每行范围后读取，任何不确定返回 unknown。 |
| elapsed_clock | Windows GetTickCount64 与 Apple mach_continuous_time 计入休眠时间；std::time::Instant 无法保证相同的平台语义。 | 无参数时钟调用；timebase 输出是已初始化、正确 C 布局的两个 u32；使用 u128 运算并检查结果范围。 |

SQLx 状态操作、归档事务、网络与 CLI 逻辑没有 unsafe。公共私有文件机制应使用 Foundation，Windows 产品层只保留实际 Sunshine 数据库和行政状态选择。共享 worker future 桥不创建产品 Tokio runtime；此审查记录当前必要性；原生 CI 必须针对最终 Source，不能从 Linux 成功推断 Windows 权限已通过。

最终共享 Client 输入为 0.10.5 / a3c827b7f0f69d84ff69f72be1533a7171ab63d6；该 Source 的原生 CI 与正式发行分别核验，产品仍按自身最终 Source 再跑实际平台验证。
中性日志输入为 0.11.2 / d58b9ef0822984ee0d29fb8b8139cfd2787374fb，精确 Git 来源与 Cargo.lock 一致。

0.5.3 消费公共 Unix 锁守卫的显式解锁再关闭：产品的维护锁、私有状态和执行 journal 使用独立打开的锁文件，不让 fork 至 exec 之间仍存活的重复描述符延长旧守卫锁。持久锁 inode、现有副作用事实与竞争拒绝不变；产品没有新增 unsafe 或绕过权限的分支。Windows 存储继续使用 `for_service_account(S-1-5-18)` 的 LocalSystem protected 策略，日志仍使用中性日志当前用户策略；二者没有选择唯一 LocalService SCM SID 的继承模式，故无需改变日志输入或用户 ACL。公共库原生回归与产品实际安装/服务验收分别记录。

macOS 服务控制继续复用公共 CLI：成功 `bootout` 后在原 deadline 内确认 launchd 定义实际卸载，避免 `SIGTERMed` 被当作已卸载。新的公共 Source 只修正该同步边界；产品没有复制管理器逻辑、改变启停策略或新增 unsafe。真实用户域 helper 回归不替代本产品 system 域服务生命周期，正式验收按最终公共与产品 Source 分别执行。

Linux 安装器在创建临时封装根后显式设置 0755；实际 DEB 回归读取 data.tar 的根和全部目录，检查 0755 与 root:root，防止临时目录的 0700 成为系统路径载荷元数据。此构建修正没有新增 unsafe，也不改变运行时私有状态的 0700 合同。本机缺少 dpkg-deb 时明确跳过真实 DEB 用例；最终 Source 的 Linux 原生测试和正式产物元数据需分别确认。

## 当前工程约束

正式状态以 Git tag、最终 Source 工作流和 Release 产物为准。Rust 1.99.0 是截至 2026-10-07 的当前正式版；Tokio 选择稳定的 ~1.53.2，兼容补丁由根 Cargo.lock 锁定。unsafe function 内的原始解引用和 foreign 调用必须放进显式 unsafe 块（unsafe_op_in_unsafe_fn = deny）。这项约束检查操作边界，不替代原生 ABI、权限与生命周期验证。正式输入和用户数据身份分开记录，不通过发行号推导持久状态。

产品协议 1.0.0 固定官方 Git revision f7943abee4bb74eb6b2a86687ca1ce28028070d9，可独立构建并由正式打包门禁核对 manifest 与 lock 的相同来源。通信身份 `sunshine-management/1` 与任务身份 `sunshine-task/1` 独立，使用各自的合同名称和指纹；不保留按发布版本切换能力的运行分支。Bootstrap 示例按规范位于 config/，打包名称保持 bootstrap.example.json。

## 统一规范验收边界

| 适用条款 | 当前实现与本轮验收 | 真实限制 |
|---|---|---|
| 2–5、19：职责、目录与身份 | 单Rust包保留adapter、engine、journal、provisioning、transport、CLI和平台职责；bootstrap样例归config，deploy只放服务部署。软件1.0.0、sunshine-management/1、sunshine-task/1、journal fingerprint分别定义。 | 真实官方Git输入和精确1.0.0已固定；package gate验证相同lock来源，拒绝path和浮动输入。 |
| 6–9：结构、安全和CLI | capabilities必填configuration_overwrite、pending_pairing_listing，由实际执行实现报告true，不用Client发行号推导能力；bootstrap和identity严格当前结构。sunshine_version必填，不以缺字段推测当前观察值。 | 无历史来源猜测或运行时转换；旧/未知结构明确拒绝并保全，显式pair replace先验证重要journal。 |
| 11–14：副作用、预算和恢复 | Task形状未变，固定Task/3保持已持久fingerprint；EffectIntent在副作用前写入，unknown save不重发；同ID不同fingerprint拒绝。双pipe、HTTP、WebSocket、日志page和持久journal有预算。 | 直接进程deadline不宣称终止所有独立后代；真实Sunshine服务、WebPKI WSS与驱动/安装器需对应环境。 |
| 15、20–23：日志、发行和验证 | 中性typed日志及产品实例事件；native包校验Source、版本、hash、权限/服务身份。Mac Rust及Python包/恢复夹具通过，新增immutable protocol input+lock gate负例。 | loopback自签HTTPS测试与标准WebPKI WSS gate分别执行；Windows/MSI、Linux/deb及真实服务不由Mac模拟替代。 |

1.0.0 消费正式 Foundation Client 0.10.5 的有界子进程退出等待修复；公共层已通过原生 CI 和源码资产核验。产品网络 doctor 增加调用者期限，凭据及版本错误准确报告本机 HTTPS 回环证书策略。产品没有新增 unsafe、权限分支或数据迁移；发行门禁按最终产品 Source 执行原生安装与服务验收。
