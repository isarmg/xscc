# RustDesk 串流分支

## 项目简介

基于 RustDesk 的精简串流工具，围绕屏幕、系统声音与键鼠交互开发，以高帧率、低延迟为改造目标。主机端与客户端使用 Flutter 界面。

## 项目功能

- 屏幕共享与系统音频传输
- 键鼠控制、文字剪贴板和截图保存
- 局域网发现、IP/域名直连
- 首次指纹确认、强制加密与设备权限控制
- 独立的 60、90、120 FPS 串流目标，以及积压时的画面呈现合并
- 保留现有硬件编解码和 GPU 纹理路径

## 适用平台

- Windows 11：x86_64（Intel/AMD 64 位）
- Linux：x86_64，Wayland 桌面
- macOS 26：Apple Silicon
- iOS 26 与 iPadOS 26，仅控制端
- Android：ARM64，Android 10（API 29）及以上，仅控制端

硬件按采集、编解码与渲染能力选择，不按型号发布时间判断。

## 快速部署

本分支仅支持在实体设备的操作系统上原生安装和运行，不提供 Docker、Compose 或 Kubernetes 部署方式。

当前重构分支尚未提供经验证的安装包。上游 RustDesk 安装包不包含本分支的精简和串流改动。

## 编译部署

克隆仓库，按 `.github/workflows/flutter-build.yml` 准备固定版本的 Rust、Flutter、原生依赖与桥接生成产物，再在仓库根目录执行：

```sh
python3 build.py --flutter
```

硬件编解码构建需使用对应平台工作流中的功能参数。Linux 需要运行中的 Wayland 桌面及其屏幕共享、远程输入后端。CI 中的容器仅用于编译和打包，生成的应用需在实体设备上原生运行。

[项目文档](docs/streaming.md)
