# XSCC GPUI 0.1.0-beta.2

控制端和被控端采用新的设备工作台，统一授权设备卡片、绑定表单、状态反馈和深浅主题，并改用中文系统字体。桌面及移动端移除临时 IP 直连入口；本次候选包只提供受管连接。受管访问地址上报、会话授权、屏幕选择和帧率控制保留。

受管客户端的加密和证书依赖同步升级至修复版本：ctutils 0.4.4、rustls-webpki 0.103.16、webpki-root-certs 1.0.10，对应 [RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html)、[RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html) 和 [RUSTSEC-2026-0383](https://rustsec.org/advisories/RUSTSEC-2026-0383.html)。

| 平台 | 产物 |
| --- | --- |
| Windows 11 x86_64 | 控制端安装包/便携包、被控端安装包 |
| Linux x86_64 / Wayland | 控制端与被控端归档 |
| macOS 26 Apple Silicon | 控制端与被控端应用归档 |
| Android ARM64 / API 29+ | 控制端 APK，CI 测试签名 |
| iOS/iPadOS 26 ARM64 | 控制端未签名 IPA，需要自行签名与配置 provisioning |

所有发布产物来自同一源码提交的成功原生构建，并提供 `SHA256SUMS` 和 `release-provenance.json`。Windows/macOS 产物未签名；Windows 被控端请使用安装包，文件归档不替代系统服务安装。

首次使用需准备 xscs 服务端地址和对应实例密码，并在管理端分配控制授权。正式部署使用 HTTPS/WSS，同机测试支持 `http://127.0.0.1:18104`、`http://localhost:18104` 及对应 WS 连接。本版本不包含临时 IP 测试监听。已安装的 Windows 被控端升级前需关闭界面并卸载旧版，身份与配置按现有卸载约定保留。

这是实验分支预发布。CI 的编译、单元测试和打包检查不代替物理设备的界面、捕获、声音、权限与跨设备串流验收；移动端仅提供控制端，不提供被控服务。
