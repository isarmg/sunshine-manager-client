# Sunshine Client

Sunshine 的独立本机管理代理，公开入口为 `sunshine-client`。不处理视频流，不安装或停止 Sunshine 本体，不索取屏幕录制/输入控制权限。后台使用系统服务，删除了 C# 托盘与第二套配对向导。

当前纯 CLI 与系统服务版本为 `0.1.0-rc.9`；用法和验收限制见 [CLI 改造说明](docs/releases/cli-unreleased.md)。安装产物未签名、未公证，实机与升级验收边界见发行说明。

支持 Windows x64、Linux x64 和 macOS Apple Silicon（arm64）。不再为 Intel macOS 适配、运行 CI 或提供发行包。

各平台安装、覆盖修复、配对和 HTTPS 证书配置见 [完整配置指南](docs/platform-setup.md)。每个平台只发布一个原生安装包；不再在发行归档中附带 PowerShell、Shell 或 Python 安装包装脚本。

## 初次配置

```sh
sunshine-client setup
```

Unix 写操作使用 `sudo`，Windows 使用管理员终端。有交互终端的原生安装器会在部署提交后直接调用 `setup`；静默或无终端安装只完成部署并保留进度。`setup` 会检查已有身份并恢复未完成事务，询问是否启用开机运行、立即启动服务和验证连接。配置、配对持久化、服务注册、启动策略、运行状态与连接均逐关卡复查，只有当前关卡验证成功才进入下一关；失败结果包含稳定的 `error.code`、`error.step`、可读 `error.message`，服务管理器失败还包含受限长度的 `error.detail`。配对失败保留安装、身份和安全进度，再次运行 `setup` 即可继续；后台 `run` 不会隐式配对或生成第二个守护进程。

配对只需 Server HTTPS origin、配对码、本机 Sunshine HTTPS 地址及凭据；内部 Manager/Device ID 由 Server 解析。Sunshine 地址沿用现有策略：**HTTPS 回环 IP 字面量**，如 `https://127.0.0.1:47990/`，证书必须受实际运行账户信任且名称匹配。没有 `--insecure`，也不自动接受自签名证书。

自动化运行 `sunshine-client setup --input-stdin --non-interactive --format json`，由部署器通过 stdin 交付严格 JSON：

```json
{
  "server": "https://manager.example.com",
  "pairing_code": "<由受保护输入提供>",
  "sunshine_endpoint": "https://127.0.0.1:47990/",
  "sunshine_username": "<由受保护输入提供>",
  "sunshine_password": "<由受保护输入提供>",
  "restart_allowed": false
}
```

不要把此文档中的占位符当作有效配对码，也不要将秘密写入参数或部署日志。超时或丢回执后用 `status`、`pair resume` 核对同一安装身份；后台 `run` 不再自动注册或恢复配对。

## 修改本机设置和密码

配置候选文件仅允许 `sunshine_endpoint`、`restart_allowed`。`config validate/diff/apply --file /absolute/candidate.json` 使用同一规则，提交还需 `--expected-revision`。

```sh
sunshine-client service stop
sunshine-client credentials update --interactive
sunshine-client doctor --sunshine
sunshine-client service start
```

密码更新保留 Manager 绑定、installation ID、设备长期凭据和执行日志。自动化凭据文档仅接受 `sunshine_username`、`sunshine_password`。重配对不能通过删除状态目录实现；先远端退役、归档受保护身份与执行记录，再采用新实例。

`tasks list` 和 `tasks show op_<uuid>` 读取本地执行事实，不代表 Server 完整任务生命周期。没有清除去重、改写成功或任意命令重放入口。

## 路径、构建与安装

当前 `--config` 选择整个受保护状态目录，`--state` 是兼容别名，不能同时使用。默认目录：Windows `C:\ProgramData\SunshineClient`，Linux `/var/lib/sunshine-client`，macOS `/Library/Application Support/sunshine-client`。设备身份物理格式仍保持原有 Unix 文件 / Windows SQLite 后端。

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

Linux DEB 面向 Ubuntu 24.04 amd64；Windows MSI 保留 WiX 构建工具；macOS 原生安装包只安装 Client 的低权限系统 LaunchDaemon。默认卸载保留凭据和执行日志，并在 Manager 退役设备。安装、配置、配对和服务操作均由 `sunshine-client` 完成。

原生验收范围与精确基线见 [CLI 兼容矩阵](docs/releases/cli-compatibility.md) 和 [验证记录](docs/releases/cli-unreleased.md)。
