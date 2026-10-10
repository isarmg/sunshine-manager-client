# 在 Linux 上使用 xscc

适用范围：Ubuntu 24.04 x86_64。其他平台见[平台入口](../platform-setup.md)。

## 安装并配对

先完成[安装前准备](../platform-setup.md#开始前准备)。从 [1.0.0 Release](https://github.com/isarmg/xscc/releases/tag/v1.0.0) 下载 `xscc_1.0.0_amd64.deb` 与 `xscc_1.0.0_amd64.deb.sha256`。

进入下载目录，先核对安装包摘要：

```sh
sha256sum --check --strict xscc_1.0.0_amd64.deb.sha256
```

预期校验结果为 OK。随后执行：

```sh
sudo apt install ./xscc_1.0.0_amd64.deb
sudo xscc service stop
sudo xscc setup --interactive
sudo xscc status --check --format json
sudo xscc doctor --sunshine --format json
```

向导各项输入与成功标准见[向导输入与完成结果](../platform-setup.md#向导输入与完成结果)。随后按[日常使用](../usage.md)核对服务、绑定和管理任务结果。

## 日志与本机状态

服务为 `xscc.service`，运行账户为 `xscc`。查看系统日志：

```sh
sudo journalctl -u xscc.service -n 100 --no-pager
```

默认状态目录为 `/var/lib/xscc`。`--config` 指整个目录；系统服务使用默认位置。Sunshine 服务控制面向系统级 `sunshine.service`，用户会话服务需在本机另行管理。

## 排查本机问题

- 安装失败：查看包管理器错误，核对架构、依赖、已有服务路径和目录权限。
- 服务未运行：检查上述 journal 和 `service status --format json`；完成配对后再启动。
- Sunshine 可访问但服务控制失败：核对目标是否为系统级 `sunshine.service`，以及本机服务授权。Sunshine 连接和凭据问题见[通用排障](../troubleshooting.md)。

同版 DEB 可用 `sudo apt install --reinstall ./xscc_1.0.0_amd64.deb` 修复，保留业务状态。修复后检查版本、服务与业务连接。

## 卸载

```sh
sudo apt remove xscc
```

普通卸载停止并移除 xscc 服务与程序，保留身份、凭据和执行记录；Sunshine 自己的安装不变。客户端没有自动清除业务状态的命令。

卸载完成后，对应系统服务应不再登记。

## 构建原生安装包

在 Ubuntu 24.04 x86_64 上准备 Rust `1.99.0`、Python `3.11+`、C/C++ 构建工具和 `dpkg-deb`。从干净、已提交的仓库根目录运行，输出目录须为尚不存在的仓库外绝对路径：

```sh
python3 scripts/package-client.py --output "$HOME/xscc-output"
```

生成 `xscc_1.0.0_amd64.deb`、`xscc-1.0.0-x86_64-unknown-linux-gnu.tar.gz` 及对应 manifest / 校验文件。

正式标签构建加 `--require-tag`，要求与版本号一致的 annotated tag 精确指向 HEAD。归档用于分发二进制与文档；系统服务通过原生安装包安装。

通用工具链、质量检查和源码说明见[开发指南](../development.md)。原生安装器构建成功后，还需在对应系统验证安装、服务生命周期和真实设备行为。

## 继续使用

[配置](../configuration.md) · [日常使用](../usage.md) · [维护](../administration.md) · [通用排障](../troubleshooting.md) · [命令参考](../cli-compatibility.md)
