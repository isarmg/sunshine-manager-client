# xscc 开发与验证

## 工具链

使用 `rust-toolchain.toml` 固定的 Rust `1.99.0`、Git、Python `3.11+` 和平台 C/C++ 构建工具。Python 脚本使用标准库 `tomllib`、`hashlib.file_digest`；macOS 自带 Python `3.9` 不满足要求，先确认 `python3 --version`。`cargo` 须位于 PATH，HTTPS 测试还需要 `openssl` 和本机回环端口访问权限。

## 质量检查

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
cargo test --locked --test cli_contract -- --ignored
cargo test --locked --test local_https -- --ignored --skip authenticated_wss_delivers_result_and_revocation_stops_reconnect
```

HTTPS 测试使用本地协议夹具；真实管理端的标准信任链 WSS 联调须单独配置测试证书。发行须对最终受控依赖源码执行各平台原生 CI；本地测试不能替代原生安装、Sunshine 和端到端验收。

## 原生打包

通用入口从本机平台构建 release 二进制、原生安装器、归档、manifest 和校验文件。源码须干净且已提交，输出为仓库外尚不存在的绝对目录。

Ubuntu 24.04 x86_64 需 `dpkg-deb`：

```sh
python3 scripts/package-client.py --output "$HOME/xscc-output"
```

Windows x64 需 MSVC、.NET SDK（WiX `4.0.6` 由项目固定），在 PowerShell 执行：

```powershell
python scripts/package-client.py --output "$env:USERPROFILE\xscc-output"
```

macOS 需 Apple Silicon、Xcode 命令行工具和系统 `pkgbuild`/`productbuild`：

```sh
python3 scripts/package-client.py --output "$HOME/xscc-output"
```

正式标签构建再加 `--require-tag`，要求与版本号一致的 annotated tag 精确指向 HEAD。Windows/Linux/macOS 分别输出 MSI/DEB/未签名 PKG，安装后按[平台指南](platform-setup.md)完成设置。归档不是系统服务安装器；只包含可执行文件、文档与校验信息。

## 仓库与通信边界

本项目为单个 Rust 包。根 `Cargo.toml`/`Cargo.lock` 定义发行输入，`src/` 实现协议消费、本机适配、存储、CLI 和平台服务。`src/storage/sqlite.rs` 负责 Windows 当前 facts 表与事务，`src/cli/tests.rs` 保存 CLI 行为测试，根 `tests/` 验证网络、受保护状态与执行事实。`packaging/` 保存平台安装器与生命周期脚本，`scripts/` 保存构建、发行检查，`docs/` 保存配置与安装说明。客户端没有 Web 或独立协议包目录，协议从固定服务端源包消费。

当前使用 Rust `1.99.0`、xcsc `1.0.0` 与设备通信协议 v1。产品协议 `1.0.0` 固定官方 xscs 源码 `abe8df18d5f69a17f5d3b7f5a2b3c396b60ece9a`，由 `Cargo.lock` 核验；任务身份与副作用指纹相互独立。配置覆盖与待配对列表能力按实际实现报告。当前结构严格解码，Windows 执行事实表及 FULL 持久性事务、Unix 私有执行事实继续保全。

结果先持久化，管理端收存后压缩为去重记录；WSS 中断不取消已开始的执行。需与同步更新的管理端联调，见[管理端通信与执行恢复](https://github.com/isarmg/xscs/blob/main/docs/communication-reliability.md)。

## 配置与凭据

所有当前平台支持的管理能力均启用，没有功能开关。完整设置询问开机自启（默认 Yes），随后启动并验证连接。无交互部署通过受保护 stdin JSON 执行 `setup --input-stdin --non-interactive`，字段、回环限制、候选配置、密码更新和授权码轮换见[配置指南](configuration.md)。

`setup --interactive` / `pair --interactive` 的实例授权码为明文输入；Sunshine 密码使用隐藏输入。两者不写入日志或命令参数。不要自行把凭据加入 Shell 历史、脚本或日志；任务或账户异常时遵循平台指南保全执行事实。

公共接口与平台边界见[公共支撑](common-support.md)。代码采用 [Apache License 2.0](../LICENSE-APACHE)。
