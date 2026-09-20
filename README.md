# Sunshine Client

`sunshine-client` `0.2.6` 是 Sunshine Manager 的本机管理代理。它主动连接 Manager，读取和修改本机 Sunshine 配置、应用、日志与维护状态，并通过固定的平台服务适配器控制已安装的 Sunshine 服务；它不代理媒体流，也不安装 Sunshine。

当前支持 Windows x64、Ubuntu 24.04 x64 和 macOS Apple Silicon，固定适配 Sunshine `v2026.914.233613`。安装包签名与实机验收边界以对应 Release 为准。

## 配置概览

安装后先停止后台服务并初始化本地设置：

```sh
sunshine-client version --format json
sudo sunshine-client service stop
sudo sunshine-client config init
sudo sunshine-client config show --format json
```

准备只允许管理员读取的 Bootstrap JSON，通过 stdin 完成配对，再启动服务：

```sh
sudo install -m 0600 /dev/null /root/sunshine-bootstrap.json
sudoedit /root/sunshine-bootstrap.json
sudo sh -c 'exec sunshine-client pair --input-stdin --non-interactive --format json < /root/sunshine-bootstrap.json'
sudo sunshine-client pair status --format json
sudo sunshine-client service enable
sudo sunshine-client service start
sunshine-client status --check --format json
sunshine-client doctor --sunshine --format json
```

Bootstrap JSON、本机 Sunshine 地址约束、候选配置的 `validate/diff/apply`、密码更新和授权码轮换见[完整配置指南](docs/configuration.md)。不要把 Manager 授权码或 Sunshine 密码写进命令参数、Shell 历史和日志。

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
