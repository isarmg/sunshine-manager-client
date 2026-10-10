# 在 macOS 上使用 xscc

适用范围：Apple Silicon arm64。其他平台见[平台入口](../platform-setup.md)。

## 安装并配对

先完成[安装前准备](../platform-setup.md#开始前准备)。从 [1.0.0 Release](https://github.com/isarmg/xscc/releases/tag/v1.0.0) 下载 `xscc-1.0.0-macos-arm64-unsigned.pkg` 与同名 `.sha256`。

PKG 未签名、未公证。核验来源与摘要后，按系统提供的批准流程安装。

在下载目录核对 PKG 摘要，与对应 `.sha256` 中同名行比较：

```sh
shasum -a 256 xscc-1.0.0-macos-arm64-unsigned.pkg
```

一致后安装并设置：

```sh
sudo installer -pkg ./xscc-1.0.0-macos-arm64-unsigned.pkg -target /
sudo /usr/local/bin/xscc service stop
sudo /usr/local/bin/xscc setup --interactive
sudo /usr/local/bin/xscc status --check --format json
sudo /usr/local/bin/xscc doctor --sunshine --format json
```

macOS 的 Sunshine 服务启停由本机管理；xscc 可使用其管理 API。

向导各项输入与成功标准见[向导输入与完成结果](../platform-setup.md#向导输入与完成结果)。随后按[日常使用](../usage.md)核对服务、绑定和管理任务结果。

## 日志与本机状态

launchd 服务为 `org.sarmg.xscc`，运行账户为 `_xscc`。命令入口为 `/usr/local/bin/xscc`。

```sh
sudo /usr/local/bin/xscc service status --format json
sudo tail -n 100 /var/log/xscc.log
```

默认状态目录为 `/Library/Application Support/xscc`；`--config` 指整个目录，系统服务使用默认位置。

## 排查本机问题

- 找不到命令：使用本页的 `/usr/local/bin/xscc` 绝对路径。
- PKG 安装失败：查看 `/var/log/install.log`，核对 Apple Silicon 架构、账户及已有安装路径；使用系统提供的未签名软件批准流程。
- Sunshine 服务启停：在本机管理 Sunshine 服务；xscc 可使用其管理 API。连接、凭据和版本问题见[通用排障](../troubleshooting.md)。

修复时重新执行本页已校验 PKG 的 `installer` 命令，保留业务状态。完成后检查版本、服务与业务连接。

## 卸载

取得与已安装版本相匹配的源码，进入仓库根目录执行：

```sh
sudo sh deploy/macos/uninstall-macos.sh
```

卸载脚本没有随 PKG 安装。它停止并移除 xscc 服务和程序，保留身份、凭据及执行记录；Sunshine 自己的安装不变。客户端没有自动清除业务状态的命令。

卸载完成后，对应系统服务应不再登记。

## 构建原生安装包

在 Apple Silicon Mac 上准备 Rust `1.99.0`、Python `3.11+`、Xcode 命令行工具和系统 `pkgbuild` / `productbuild`。先用 `python3 --version` 确认版本；系统自带的 Python `3.9` 不满足脚本要求。从干净、已提交的仓库根目录运行，输出目录须为尚不存在的仓库外绝对路径：

```sh
python3 scripts/package-client.py --output "$HOME/xscc-output"
```

生成 `xscc-1.0.0-macos-arm64-unsigned.pkg`、`xscc-1.0.0-aarch64-apple-darwin.tar.gz` 及对应 manifest / 校验文件。

正式标签构建加 `--require-tag`，要求与版本号一致的 annotated tag 精确指向 HEAD。归档用于分发二进制与文档；系统服务通过原生安装包安装。

通用工具链、质量检查和源码说明见[开发指南](../development.md)。原生安装器构建成功后，还需在对应系统验证安装、服务生命周期和真实设备行为。

## 继续使用

[配置](../configuration.md) · [日常使用](../usage.md) · [维护](../administration.md) · [通用排障](../troubleshooting.md) · [命令参考](../cli-compatibility.md)
