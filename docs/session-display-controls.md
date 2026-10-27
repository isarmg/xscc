# 会话码率、分辨率与虚拟显示器

本轮从 `896e5f9e4` 开始实现，下一批原生候选标为 `0.1.0-beta.9`，Android versionCode 为 9。旧的 beta.8 Release 与已安装程序不会被源码修改自动替换。

## 操作

- 桌面控制端在会话侧栏“画质与显示”调节，移动端在会话显示面板调节。面板不覆盖远程画面，悬浮鼠标三键和原有触控坐标路径保持不变。
- 编码码率上限支持自动、500 kbps、1/2/4/8/16/32 Mbps。上限参与真实编码器目标计算，帧率与画质重算后仍保留；自动模式释放本连接的限制，继续使用原有自适应。接收流量与编码目标分别显示，不能把编码目标上限解释成包含音频、协议开销及瞬时突发的逐包限速。
- 分辨率选项来自当前远端显示器声明的模式列表，修改远端显示模式。它与本地等比显示缩放是两个不同操作。设置前释放已有按键、拖拽及触点；请求成功后清旧画面并刷新，以首个新帧恢复画面和坐标输入。Mac Retina 模式用系统逻辑分辨率展示，视频与鼠标仍使用原有物理帧坐标。
- Windows 被控端支持添加虚拟屏并从显示器列表选取。当前 Amyuni 后端最多四个虚拟屏，移除操作明确针对最近添加的虚拟屏，不把计数当作物理屏幕编号。原有“保留最后一块显示器”规则继续生效。
- 控制端成功绑定、或首次读取到已有绑定时，自动收起绑定区域；仍可手动展开查看。失败保留原输入，后续轮询不覆盖手动展开状态。桌面被控端工作台保持原有展示路径。

## 能力与失败反馈

新码率与显示控制使用明确的主机能力协商。旧主机保留原有观看和显示器切换功能，新调节项提示升级被控端，不根据应用版本字符串猜测能力。码率字段为向后兼容的可选字段：缺失保持原值，零恢复自动；公开入口接受 64 至 100000 kbps 的非零限制。

设置请求入队不等于已生效。界面按主机确认和实际显示信息更新选中值，系统或驱动拒绝请求时显示非致命错误。编码器拒绝码率设置时恢复自动并报告原因，不持续以同一个失败限制重启。

同一编码流被多个控制端共享时，使用仍在线连接所设上限的最小值。恢复自动或断开连接即释放该连接的上限。当前实际编码目标由主机遥测展示，可能因网络自适应或其它观看者的限制而低于本连接所选上限。

分辨率和虚拟显示器操作要求当前管理授权与键鼠控制权限；驱动与系统模式操作在阻塞任务中执行，不持有全局 QoS 锁。错误反馈不会关闭整个会话。未改变身份存储、管理证明、服务器地址策略或视频/触控协议坐标。

Windows Host 包只复制固定来源、摘要校验通过的 Amyuni x64 驱动文件。明确添加虚拟屏时按需走 Windows 正常驱动安装路径；现有的授权无头连接自动补屏流程也可能启用驱动，此流程不要求键鼠控制权限。已安装的兼容驱动继续复用。驱动原始许可包含广告页面条款，界面、文档和包内许可均告知。打包校验检查归档、五个分发文件与有效 catalog 签名；不安装证书、不修改驱动签名策略。来源记录随包放在 `usbmmidd_v2/driver-source.json`。详见 [Windows 打包说明](gpui-windows-beta.md)。

## 既有运行路径的必要改动

| 现有文件 | 改变的路径与原因 |
| --- | --- |
| `src/gpui_session.rs` | 原生会话事件、能力投影、显示确认与码率遥测接入，供两种原生界面使用；新逻辑在 `gpui_session/controls.rs`。 |
| `src/ui_session_interface.rs`、`src/client/io_loop.rs` | 将主机媒体设置确认转交 GPUI；其它前端保持默认空回调。 |
| `libs/base/protos/message.proto`、`libs/base/src/session_protocol.rs` | 可选码率字段、明确能力与外部数值验证；已有字段和编号不变。 |
| `src/server/connection.rs` | 控制消息调用 feature 模块，验证当前权限、后台执行并回报错误；旧自动画质逻辑保持。 |
| `src/virtual_display_manager.rs` | 新虚拟屏启用失败时保留系统驱动错误原因，便于通过现有非致命反馈诊断；驱动算法保持原实现。 |
| `src/server/video_qos.rs` | 每个现有观看者保存可选上限，复用连接退出清理与共享编码流。 |
| `src/server/video_service.rs` | 初始化和既有 QoS 检查时应用上限，后端拒绝时清理失败限制并回报。 |
| `libs/scrap/src/common/codec.rs` | 编码器增加可选限制接口及真实 VP9/AV1 回归，不支持的后端明确失败。 |
| `libs/scrap/src/common/vpxcodec.rs`、`aom.rs`、`hwcodec.rs`、`vram.rs` | 仅在显式上限启用时约束真实整数目标；恢复自动及后续 FPS/画质重算同步更新。 |
| `experiments/gpui-validation/src/client.rs` | 薄调用会话设置、成功绑定/初始已有绑定收起、保留手动展开；设置视图隔离在子模块。 |
| `experiments/gpui-validation/src/remote_video.rs` | 接收设置事件、显示确认、分辨率变更前释放输入；新请求处理在子模块。 |
| `experiments/gpui-mobile-management/src/session_view.rs` | 将显示面板动作映射到真实会话 API，保留原输入和图像布局；权限逐次检查。 |
| `experiments/gpui-mobile-management/src/session_chrome.rs` | 现有显示面板增加设置控件及能力提示；不改变视频区域几何。 |
| `experiments/gpui-mobile-management/src/panel.rs` | 成功绑定及初始已配置状态折叠，失败不清空输入。 |
| `.github/scripts/package-gpui-windows.ps1`、`packaging/gpui-windows/host-installer.nsi` | 仅 Host 准备并复制已校验驱动；卸载只移除已知随包文件，不移除其它应用可能共用的已注册驱动。 |
| `packaging/gpui-windows/README-host.txt`、`THIRD_PARTY_NOTICES.txt`、`docs/gpui-windows-beta.md` | 更新功能边界、驱动来源、原始许可及安装行为说明。 |
| `.github/workflows/gpui-windows-beta.yml`、`.github/scripts/validate-gpui-unix.sh` | 原生流程增加真实编码器码率测试并拒绝空过滤；主机现有测试已经覆盖新增 QoS 与会话测试。 |
| `.github/workflows/gpui-unix-beta.yml`、`packaging/gpui-android/app/build.gradle` | 与 Windows 候选一起标记为 beta.9，避免复用已发布 beta.8 标签。 |

## 验证边界

本地管理包检查、GPUI 主机测试、软件编码器回归、Windows 驱动打包及签名验证分别记录实际结果。完整硬件编码、VRAM、Windows 模式变更以及 Android/iOS 条件编译通过对应原生 CI 验证；构建成功不代表物理设备实测通过。未经真实验收不宣称码率精度、画质、帧率、驱动兼容或端到端延迟收益。
