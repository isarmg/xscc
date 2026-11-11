# XSCC 1.0.1

- 修复直连视频路径中人为限制为解码帧率 90% 的问题。
- 高刷新率会话不再被客户端逐帧确认串行阻塞，并改进 Windows 120/240Hz 帧节拍精度。
- 修复 Windows 输入线程异常退出后可能重新锁定系统输入的问题，确保鼠标和键盘能够恢复。

本 Release 包含 Windows 11 x86_64、Linux x86_64、macOS 26 Apple Silicon、Android ARM64
以及 iOS/iPadOS 26 ARM64 的原生 GPUI 客户端和桌面被控端候选包。Windows/macOS 包未作发行签名，
iOS IPA 需自行签名。

所有发布包必须来自同一提交，并通过对应平台的原生构建、回归测试和完整性校验。
