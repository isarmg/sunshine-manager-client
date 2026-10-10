# xscc 使用文档

xscc 是 Sunshine 主机上的本机管理代理，以系统服务主动连接 xscs。这里从安装一台主机开始，介绍当前 1.0.0 的使用和维护。

## 选择平台

- [Linux](platforms/linux.md)：Ubuntu 24.04 x86_64 DEB
- [Windows](platforms/windows.md)：Windows 11 x64 MSI
- [macOS](platforms/macos.md)：Apple Silicon arm64 PKG

各桌面平台页包含安装、构建和本机排障。首次安装需要的资料及通用向导输入见[安装入口](platform-setup.md)。

## 安装之后

1. [确认运行](usage.md)：核对服务、绑定及只读管理任务结果。
2. [调整配置](configuration.md)：修改设置或准备自动化输入。
3. [日常维护](administration.md)：服务管理和凭据更新。

## 按任务查找

- [日常维护](administration.md)：服务启停、自启、密码更新和任务记录
- [排查问题](troubleshooting.md)：网络、认证与运行问题
- [命令参考](cli-compatibility.md)：命令、输出和状态路径
- [开发指南](development.md)：共用工具链、测试和源码结构
- [公共库](common-support.md)与[原生接口审查](unsafe-audit.md)
- [1.0.0 发行说明](releases/1.0.0.md)

管理端的操作流程见 [xscs 使用指南](https://github.com/isarmg/xscs/blob/main/docs/usage.md)，通信与持久结果见[执行可靠性](https://github.com/isarmg/xscs/blob/main/docs/communication-reliability.md)。
