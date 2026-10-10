# xscc 平台安装入口

按主机平台打开对应指南。每页集中说明该平台的安装、源码构建、日志、常见问题和卸载；通用向导输入与完成结果在本页下方。

## Linux

[打开 Linux 指南](https://github.com/isarmg/xscc/blob/main/docs/platforms/linux.md)：Ubuntu 24.04 x86_64 的 DEB；使用系统级 Sunshine 服务。

## Windows

[打开 Windows 指南](https://github.com/isarmg/xscc/blob/main/docs/platforms/windows.md)：Windows 11 x64 的 MSI、管理员 PowerShell 和系统服务。

## macOS

[打开 macOS 指南](https://github.com/isarmg/xscc/blob/main/docs/platforms/macos.md)：Apple Silicon arm64 的未签名 PKG、launchd 和本机日志。

二进制 `.zip` / `.tar.gz` 归档只包含程序、说明和校验信息。安装系统服务请使用对应 MSI / DEB / PKG；归档内本页链接可直接打开在线完整指南。

## 开始前准备

准备 [xscs 管理员](https://github.com/isarmg/xscs/blob/main/docs/instance-management.md)提供的 HTTPS 根地址和实例授权码，使用管理员终端完成安装与设置。

先安装 Sunshine `v2026.914.233613`，在其 Web UI 设置用户名和密码。准备本机 HTTPS 回环地址，通常是 `https://127.0.0.1:47990/`。这个密码用于本机 Sunshine API，实例授权码用于远程管理端。Sunshine 上游安装说明见[Getting Started](https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2getting__started.html)。

下载入口：[1.0.0 Release](https://github.com/isarmg/xscc/releases/tag/v1.0.0)。各平台指南列出准确的原生包和校验文件名。安装前核对摘要；已发布包的源码身份以对应 manifest 和 `version` 输出为准。

## 向导输入与完成结果

向导依次询问管理端地址、实例授权码、本机 Sunshine 回环 HTTPS 地址、用户名和密码。首次设置会创建缺少的配置，随后询问开机自启（默认 Yes），启动服务并验证连接。

授权码在终端中明文回显，Sunshine 密码隐藏输入；请在受保护终端输入。将 HTTPS 地址填为根地址，例如 `https://manager.example.com`。远程证书应被服务账户的系统信任库信任且匹配域名。

成功后到 xscs 管理台确认设备在线，读取一项配置或状态并看到任务结果。配对中断时先查看 `xscc pair status --format json`，再用 `pair resume` 继续现有事务。

## 后续维护与卸载

完成安装后查看[日常使用](https://github.com/isarmg/xscc/blob/main/docs/usage.md)、[配置指南](https://github.com/isarmg/xscc/blob/main/docs/configuration.md)和[日常维护](https://github.com/isarmg/xscc/blob/main/docs/administration.md)。连接与配对问题见[通用排障](https://github.com/isarmg/xscc/blob/main/docs/troubleshooting.md)，安装器修复和卸载步骤见上方对应平台指南。
