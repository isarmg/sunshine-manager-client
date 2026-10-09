# xscc

`xscc` `1.0.0` 是 xscs 的本机管理代理。它主动连接 Manager，读取和修改本机 Sunshine 配置、应用、日志与维护状态，并通过固定的平台服务适配器控制已安装的 Sunshine 服务；它不代理媒体流，也不安装 Sunshine。

`1.0.0` 使用 Rust 1.99.0 和 Foundation Client 1.0.0，通信契约 1 按实际实现报告配置覆盖与待配对列表能力；任务身份与副作用指纹保持独立。当前结构严格解码，Windows facts 表和 FULL 事务、Unix 私有执行事实继续保全。发行按最终受控依赖源码执行各平台原生 CI。

当前支持 Windows x64、Ubuntu 24.04 x64 和 macOS Apple Silicon，固定适配 Sunshine `v2026.914.233613`。安装包签名与实机验收边界以对应 Release 为准。

## 配置概览

安装原生包后运行完整设置：

```sh
xscc version --format json
sudo xscc setup
xscc status --check --format json
xscc doctor --sunshine --format json
```

当前平台支持的管理功能全部启用，不提供功能开关。设置只询问开机自启，默认 Yes，直接按 Enter 即可；
配对完成后自动启动服务并验证连接。Linux 使用系统级 systemd，Windows 使用 SCM 系统服务。
无交互部署使用受保护 Bootstrap JSON：`setup --input-stdin --non-interactive`，同样自动启用、启动并验证。

Bootstrap JSON、本机 Sunshine 地址约束、候选配置的 `validate/diff/apply`、密码更新和授权码轮换见[完整配置指南](docs/configuration.md)。不要把 Manager 授权码或 Sunshine 密码写进命令参数、Shell 历史和日志。

使用 `setup --interactive` 或 `pair --interactive` 时，Manager 实例授权码按普通文本输入并在终端中明文
显示，不提供遮罩或隐藏切换；本机 Sunshine 密码仍使用隐藏输入。这两个字段都不会写入日志或命令参数。

各平台安装和状态目录见[平台安装指南](docs/platform-setup.md)。

## 开发验证

需要 Rust `1.99.0`（由 `rust-toolchain.toml` 固定）和 Python `3.11` 以上；构建脚本使用标准库 `tomllib` 和 `hashlib.file_digest`。macOS 系统自带的 Python `3.9` 不满足要求，请先确认 `python3 --version`，并使用符合要求的解释器。`cargo` 需位于 PATH；使用 rustup 安装后按其提示加载 Cargo 环境。HTTPS 测试还需要 `openssl` 和本机回环端口访问权限。

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
cargo test --locked --test cli_contract -- --ignored
cargo test --locked --test local_https -- --ignored --skip authenticated_wss_delivers_result_and_revocation_stops_reconnect
```

上述 HTTPS 测试使用本地协议夹具；真实 Manager 的标准信任链 WSS 联调需要单独配置测试证书。

## 文档

- [文档总览](docs/README.md)
- [完整配置指南](docs/configuration.md)
- [分平台部署、配对、服务管理与卸载](docs/platform-setup.md)
- [CLI 兼容矩阵](docs/releases/1.0.0.md)

代码采用 [Apache License 2.0](LICENSE-APACHE)。

当前设备通信使用协议 v1，业务任务保持独立的既有身份，需与同步更新的 Manager 联调。产品协议 1.0.0 固定官方上游源码 f7943abee4bb74eb6b2a86687ca1ce28028070d9，并由 Cargo.lock 验证输入。结果先持久化、收存后压缩为去重记录；WSS 中断不取消已开始的执行。详见 [Manager 通信与执行恢复](https://github.com/isarmg/xscs/blob/main/docs/communication-reliability.md)。

## 仓库布局

本项目为单个 Rust 包，根 `Cargo.toml`/`Cargo.lock` 定义发行输入，`src/` 实现协议消费、本机适配、存储、CLI 和平台服务。`src/storage/sqlite.rs` 负责 Windows 当前 facts 表和事务，`src/cli/tests.rs` 存放 CLI 行为测试；根 `tests/` 验证网络、保护状态和执行事实。`packaging/` 保存各平台安装器与生命周期脚本，`scripts/` 保存构建和发行检查，`docs/` 保存当前配置与安装说明。Client 没有 Web 或独立协议包目录，协议从固定 Server 源包消费。

当前发布版本：**1.0.0**。参见 [1.0.0 发布说明](docs/releases/1.0.0.md)和[项目命名](docs/naming.md)。

CLI 参数、输出与兼容性约定见 [CLI 兼容性](docs/cli-compatibility.md)。
