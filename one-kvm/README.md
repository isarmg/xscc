# XSCC One-KVM Web 被控服务

One-KVM 作为 `xscc/one-kvm` 独立模块维护，直接部署在 Linux 或 Windows 物理机器上。两种平台使用相同的 Web 页面完成设备配置和远程访问；原生 XSCC 控制端也可以按授权关系连接。运行容器镜像和上游在线更新不再属于部署方式。

一套宿主服务可绑定多个 XSCS `controlled / one_kvm` 实例。各实例分别保存被控身份及凭据、目标授权、硬件配置和运行状态；浏览器从宿主首页选择实例，进入 `/instances/<实例名>/`，配置该实例的视频源/显示器、键盘鼠标、音频及平台提供的其他设备。可用能力按实际硬件及操作系统枚举，不能把 Windows 系统输入当作独立物理键鼠或独立用户桌面。

前端沿用仓库定制 xcss-web / Leptos Rust WASM，Axum 负责 HTTP、WebSocket 和媒体授权。Linux 使用原有采集、编码、USB HID、CH9329、音频、ATX 和虚拟媒体实现；Windows 运行本机 Web 宿主，桌面采集和系统输入需要已登录的交互会话。

- [构建、裸机安装、实例绑定与维护](docs/managed-kvm-operations.md)
- [架构与授权边界](docs/managed-kvm.md)
- [本次多实例与裸机验证](docs/multi-instance-validation.md) · [历史验收记录](docs/managed-kvm-validation.md)
- [导入来源](SOURCE.json) · [前端来源](web/STACK-SOURCE.json) · [管理协议来源](crates/management/FORK.md)
- [上游中文介绍](docs/upstream-README.md) / [Upstream introduction](docs/upstream-README.en.md)

首次使用在 XSCS 为每组设备创建独立被控实例，在本机 CLI 用对应授权码逐个绑定。浏览器使用另一个 `controller / one_kvm` 实例的授权码；原生 XSCC 使用自己的 `controller / rustdesk` 实例。控制端到各被控实例的关系由 XSCS 管理员分别授权。同一浏览器控制端可登录多个获授权实例；宿主复用受保护的控制端凭据，各实例的会话与撤权独立。

从本模块构建前端与宿主，不使用上游二进制覆盖；本模块保留独立 Cargo 锁文件、依赖补丁和构建入口。整体许可证见 [LICENSE](LICENSE)，原生协议来源见 [NOTICE](src/rustdesk/NOTICE)。编译、软件回归与平台打包不能代替显示器、键鼠、音频、ATX、USB 等实机验收。
