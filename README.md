# xscc

xscc 是 xscs 的本机 Sunshine 管理代理，主动连接管理端，执行本机管理任务并持久保存执行结果。

## 项目功能

- 读取和修改 Sunshine 配置、应用，处理 Moonlight PIN
- 提供日志、诊断、维护状态与任务执行结果
- 在 Windows、Ubuntu 上控制已安装的 Sunshine 服务；不代理媒体流，也不安装 Sunshine

## 适用平台

Windows 11 x64、Ubuntu 24.04 x86_64、macOS Apple Silicon（arm64）。固定适配 Sunshine `v2026.914.233613`；macOS 不提供 Sunshine 服务启停能力。

## 快速部署

先安装受支持版本的 Sunshine，在其 Web UI 设置用户名、密码；准备 xscs 管理员提供的 HTTPS 地址和实例授权码。从 [Release](https://github.com/isarmg/xscc/releases) 下载本机平台的原生包及 `.sha256`，核对 SHA-256 后安装。

Ubuntu 24.04：

```sh
sudo apt install ./xscc_1.0.0_amd64.deb
sudo xscc service stop
sudo xscc setup --interactive
sudo xscc status --check --format json
sudo xscc doctor --sunshine --format json
```

macOS Apple Silicon：

```sh
sudo installer -pkg ./xscc-1.0.0-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc setup --interactive
sudo /usr/local/bin/xscc status --check --format json
sudo /usr/local/bin/xscc doctor --sunshine --format json
```

Windows：运行 `xscc-1.0.0-windows-x64.msi` 完成安装，再打开新的管理员 PowerShell：

```powershell
xscc service stop
xscc setup --interactive
xscc status --check --format json
xscc doctor --sunshine --format json
```

向导依次询问管理端地址、实例授权码、Sunshine 回环 HTTPS 地址（通常为 `https://127.0.0.1:47990/`）及其用户名、密码，然后设置自启（默认 Yes）、启动并验证连接。授权码明文回显，Sunshine 密码隐藏输入；不要把秘密写入命令参数。最后到管理端确认设备在线并完成一项只读任务。macOS 包未签名、未公证，校验后按系统批准流程安装。

## 编译部署

准备 Git、Rust `1.99.0`、Python `3.11+` 和平台 C/C++ 构建工具。以下在 Ubuntu 24.04 x86_64 上执行，还需 `dpkg-deb`；源码必须干净且已提交，输出目录必须是仓库外尚不存在的绝对路径：

```sh
git clone https://github.com/isarmg/xscc.git
cd xscc
python3 scripts/package-client.py --output "$HOME/xscc-output"
sudo apt install "$HOME/xscc-output/xscc_1.0.0_amd64.deb"
sudo xscc service stop
sudo xscc setup --interactive
sudo xscc status --check --format json
sudo xscc doctor --sunshine --format json
```

打包脚本执行锁定依赖的 release 构建，并输出原生安装包、归档及校验信息。Windows 原生打包另需 MSVC 和 .NET/WiX；macOS 使用 Apple Silicon 工具链及系统 PKG 工具。普通编译可运行 `cargo build --locked --release`，但单个二进制不替代系统服务安装包。

[详细文档](https://github.com/isarmg/xscc/blob/main/docs/README.md)
