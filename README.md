# Sunshine Client 0.1.0-rc.2

独立运行在 Sunshine 主机上的管理客户端，支持 Windows 11 及以上 x86_64（最低 build 22000）和 Linux x86_64 GNU。
本文描述主分支的新配对流程；已发布 rc.2 的行为以该版本 Release 说明为准，不修改历史发行包。
主动连接 Manager WSS，不代理 Sunshine–Moonlight 视频、不编码、不执行任意脚本，
不自动下载、安装、更新软件，也不开新的入站管理端口。

候选版本的安装测试不等于真实 Sunshine 验收。协议为 `sunshine-management/1`，
固定配置适配目标为官方 `v2026.516.143833`，其他版本会被拒绝。

## 下载与身份

### 引导安装（新）

Windows 构建现在生成 **MSI 安装包**，Linux 构建生成 **DEB 安装包**，原归档保留用于校验和无人值守部署。
安装包旁的 `.manifest.json` 记录完整源码提交与校验和。MSI 使用数字版本，候选版本身份以 manifest 和客户端 `--version` 为准。

- Windows：双击 MSI → 按向导安装 → 从开始菜单打开 **Sunshine Client** → 右键托盘选择“配对设置”。
  只填写服务器地址、配对码、本机 Sunshine 回环地址、端口、账号和密码，不输入设备标识或 CA 文件。
  配对设置需要 UAC 管理员授权；托盘本身不需要管理员权限。托盘下次登录自动出现并请求启动已配对服务。
  “停止服务并退出”会等待服务确实停止，权限不足时请求 UAC；取消授权或停止失败会保留托盘。
  Windows 服务改为手动启动；托盘菜单和配对窗口采用圆角。结束任务等强制终止不等于正常菜单退出。
  托盘显示的是 Windows 服务状态，真实连接、Sunshine 可达性及配置生效情况仍在 Manager 分别核验。
  在 Windows“已安装的应用”中卸载 Sunshine Client；保留设备身份和去重记录，不删除 Sunshine。
- Ubuntu 24.04 x86_64：安装对应版本的 DEB，然后运行 `sudo sunshine-client-setup`，按提示完成配对。GitHub 下载文件名以附件实际名称为准。
  密码与配对码不回显；完成后自动启用 systemd 服务。卸载使用 `sudo apt remove sunshine-client`，保留状态和服务账户。

Windows 依赖系统 .NET Framework 4.8（安装器/托盘原生验收基线 Windows Server 2025）；尚未做其他桌面系统的安装实测。
安装包没有 Authenticode 签名，不能承诺没有 SmartScreen 提示。安装不会关闭 TLS 校验、修改 Sunshine 或开放入站端口。
两端只接受系统已信任的证书，不提供 TOFU、自签名确认或关闭 TLS 校验选项。
Windows 服务使用 LocalSystem，企业 CA 应部署到本机计算机的受信任根存储，而不只是某个登录用户的个人存储。
默认 Sunshine 自签名证书若未受系统信任，或 SAN 不包含填写的回环 IP，配对会失败；此流程不自动更改 Sunshine 证书。
需要配合提供 `/sunshine-client/v1/pairing` 的新版 Server。系统信任不豁免证书名称、有效期校验。
首次安装不支持覆盖已有服务或转换旧状态。默认不授权 Sunshine 重启。

构建由 `scripts/package-client.py` 统一完成：Windows 使用系统 C# 编译器及固定 WiX 4.0.6，Linux 使用 `dpkg-deb`。
CI 在一次性 GitHub 托管主机上验证 MSI/DEB 的真实安装、初始化、服务重启、拒绝覆盖和卸载保留状态；这些测试禁止在用户主机上运行。

本仓库 Client 标签使用 `v0.1.0-rc.2` 形式，和 Server 仓库分别发行。下载请查看 [GitHub Releases](https://github.com/isarmg/sunshine-manager-client/releases)。
发行项为 Windows MSI、Linux DEB、Windows ZIP、Linux tar.gz 及各自 SHA-256 文件；归档含二进制、安装/卸载脚本、
无秘密的 bootstrap 示例、许可证、manifest 和逐文件 SHA256SUMS。
二进制 `--version` 与 manifest 绑定完整源码提交。Windows 使用静态 MSVC CRT，**没有 Authenticode 签名**。
校验和检查字节完整性，不代替独立发布者身份验证；要求 AllSigned 的环境须先完成受信任签名。
不要关闭全局脚本策略或 TLS 校验。

## 准备受保护配置

1. 在 Manager 新建实例，取得配对码。配对码可取消、只能绑定一次，没有自动有效期。
2. 如使用命令行，将 bootstrap 示例复制到新建私有目录，填写 Manager WSS URL 和配对码。
3. 本机填写 Sunshine 回环 HTTPS URL、用户名和密码；不把 Sunshine 密码交给 Manager。
4. Sunshine 证书 SAN 必须匹配 URL 的主机名/IP；信任 CA 不豁免名称、有效期验证，不跟随不可信重定向。
5. 默认 `restart_allowed: false`。只有本机管理员允许后才开启，Manager 管理员仍须逐次确认重启。

bootstrap 含秘密，不放入 Git、下载归档、聊天或日志，也不通过命令行传密码。初始化拒绝未填写的示例。
Linux 要求父目录私有、文件 0600、由执行安装的 root 所有；Windows 要求目录及文件只授权当前管理员、
Administrators 和 SYSTEM，拒绝重解析点或不受保护输入。注册成功后内部配对码清除，原始 bootstrap 由管理员确认后删除。

## Linux 安装与卸载

发行包构建及原生安装验收基线为 Ubuntu 24.04 x86_64（glibc 2.39、systemd、支持 `openat2` 的内核）。
不是 musl 静态包，不保证较旧 glibc 或其他发行版可直接运行；其他系统须自行原生构建并验收。
系统 WSS 信任使用 OpenSSL 3，DEB 声明 `libssl3t64` 和 `ca-certificates` 依赖；tar.gz 用户须自行准备这些系统依赖。
先核验官方附件校验和并解包：

```sh
sha256sum --check sunshine-client-0.1.0-rc.2-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf sunshine-client-0.1.0-rc.2-x86_64-unknown-linux-gnu.tar.gz
cd sunshine-client-0.1.0-rc.2-x86_64-unknown-linux-gnu
sudo bash install-linux.sh "$PWD/sunshine-client" /absolute/private/bootstrap.json
systemctl status sunshine-client.service
```

二进制位于 `/opt/sunshine-client`，独立低权限用户为 `sunshine-client`，状态位于 `/var/lib/sunshine-client`。
systemd 自动启动、失败重试；配置或凭据被拒绝时停止并要求本机管理员处理。安装不修改 Sunshine 或防火墙。

```sh
sudo bash uninstall-linux.sh
```

卸载移除 Client 服务和二进制，保留状态、去重记录及系统账户，并要求在 Manager 撤销设备。
已有安装、状态或服务账户使安装器拒绝继续；不提供覆盖安装或旧状态转换。

## Windows 安装与卸载

原生构建/系统服务 CI 基线为 Windows Server 2025 x86_64；桌面 Windows 的真实 Sunshine 验收另行记录。

在管理员 PowerShell 中，通过 `Get-FileHash -Algorithm SHA256` 比对发行校验文件，解包并审阅脚本，
在允许受信任本地脚本的执行策略下运行：

```powershell
.\install-windows.ps1 -Binary 'C:\ClientPackage\sunshine-client.exe' -Bootstrap 'C:\PrivateBootstrap\bootstrap.json'
Get-Service SunshineClient
```

bootstrap 必须存入新建私有目录。管理员可先对这个新目录关闭继承、仅授权管理员和 SYSTEM，再写入配置：

```powershell
icacls.exe C:\PrivateBootstrap /inheritance:r /grant:r '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-18:(OI)(CI)F'
```

检查文件没有额外授权；不要对现有公共目录递归执行权限命令。
安装器使用 `%ProgramFiles%\SunshineClient` 与 `%ProgramData%\SunshineClient`，收紧 ACL，创建手动启动的
`SunshineClient` 服务。服务以 LocalSystem 运行，属于高权限本机组件，仅执行产品白名单管理操作。

```powershell
.\uninstall-windows.ps1
```

卸载保留 ProgramData 状态，并要求在 Manager 撤销设备。中途安装失败保留局部安装供检查，不自动删除执行记录。

## 运行与构建

```text
sunshine-client --version
sunshine-client init --state ABSOLUTE_PATH --bootstrap ABSOLUTE_PROTECTED_FILE
sunshine-client pair --state ABSOLUTE_PATH
sunshine-client run --state ABSOLUTE_PATH
```

`pair` 先校验本机 Sunshine 的凭据及支持版本，再通过 Server 查询配对码归属并完成注册；仅启动服务不能当作配对成功。
初始化且尚未生成注册身份时可纠正填写错误；注册请求已发出但回执丢失时，只允许同一配置重试，不重新生成设备凭据。
`run` 用于前台受控调试，不与服务共享同一状态目录。Windows 系统服务内部另用 `service` 入口。
页面分别展示 Client 在线、Sunshine 可达和配置状态；“已保存”“待重启”“待验证”不等于运行时已生效。
配置按白名单与修订合并，任务去重；结果不确定时先核对，不盲目重复重启。Client 独立于 Sunshine 进程。

从产品仓库根使用 Rust 1.98.0、Python 3.11+；Windows 使用 MSVC，TLS 测试另需 OpenSSL：

本仓库可单独检出。Foundation Client SDK 和 Server 所属产品协议均固定完整 Git 提交与精确版本，
无需相邻 Server/Foundation 工作目录。协议源码与 Manager 实现在 [Server 仓库](https://github.com/isarmg/sunshine-manager-server)。

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -p sunshine-client --test local_https -- --ignored
python scripts/package-client.py --output /absolute/new-output
```

打包只构建 Client，要求干净且已提交源码；Windows 使用本机绝对输出路径。
`--require-tag` 另要求 annotated Client 标签指向 HEAD。双平台必须分别原生验收。
`scripts/check-client-package.py --archive ABSOLUTE_ARCHIVE --sha FULL_SOURCE_SHA` 独立校验解包与执行身份；
其 `--install` 只允许一次性 GitHub-hosted runner，不能当作用户主机上的测试清理工具。
升级/恢复继续以 `sarmg-upgrade` 的明确版本支持矩阵为准，本版不提供历史兼容。
