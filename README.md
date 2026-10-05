# Sunshine Client

`sunshine-client` `0.3.2` 是 Sunshine Manager 的本机管理代理。它主动连接 Manager，读取和修改本机 Sunshine 配置、应用、日志与维护状态，并通过固定的平台服务适配器控制已安装的 Sunshine 服务；它不代理媒体流，也不安装 Sunshine。

当前支持 Windows x64、Ubuntu 24.04 x64 和 macOS Apple Silicon，固定适配 Sunshine `v2026.914.233613`。安装包签名与实机验收边界以对应 Release 为准。

## 配置概览

安装原生包后运行完整设置：

```sh
sunshine-client version --format json
sudo sunshine-client setup
sunshine-client status --check --format json
sunshine-client doctor --sunshine --format json
```

当前平台支持的管理功能全部启用，不提供功能开关。设置只询问开机自启，默认 Yes，直接按 Enter 即可；
配对完成后自动启动服务并验证连接。Linux 使用系统级 systemd，Windows 使用 SCM 系统服务。
无交互部署使用受保护 Bootstrap JSON：`setup --input-stdin --non-interactive`，同样自动启用、启动并验证。

Bootstrap JSON、本机 Sunshine 地址约束、候选配置的 `validate/diff/apply`、密码更新和授权码轮换见[完整配置指南](docs/configuration.md)。不要把 Manager 授权码或 Sunshine 密码写进命令参数、Shell 历史和日志。

使用 `setup --interactive` 或 `pair --interactive` 时，Manager 实例授权码按普通文本输入并在终端中明文
显示，不提供遮罩或隐藏切换；本机 Sunshine 密码仍使用隐藏输入。这两个字段都不会写入日志或命令参数。

各平台安装和状态目录见[平台安装指南](docs/platform-setup.md)。

## 开发验证

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

## 文档

- [文档总览](docs/README.md)
- [完整配置指南](docs/configuration.md)
- [平台安装与升级](docs/platform-setup.md)
- [CLI 兼容矩阵](docs/releases/cli-compatibility.md)

代码采用 [Apache License 2.0](LICENSE-APACHE)。

当前设备通信使用协议 v3，需与同步更新的 Manager 配合部署。结果先持久化、收存后压缩为去重记录；WSS 中断不取消已开始的执行。详见 [Manager 通信与执行恢复](https://github.com/isarmg/sunshine-manager-server/blob/main/docs/communication-reliability.md)。
