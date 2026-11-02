# 会话码率与完整显示模式

## 2026-11-02：虚拟屏完整配置

新 Host 在会话中公告版本化的虚拟屏配置能力。Windows 被控端顶部的“显示设置”可在连接前查询、准备签名驱动并配置；桌面和移动控制端的虚拟显示器菜单可设置总数量、分辨率、精确 Hz，以及长期保留或最后一个会话正常断开后移除。策略作用于整组 XSCC 虚拟屏。

主机分别回报配置数量、PnP 实际识别数量、活动且可捕获数量、稳定屏幕标识和当前模式。操作带关联 ID，返回实际结果并先刷新屏幕列表；超时表示尚未确认，操作可能仍在 Windows 执行。新增屏幕只在可捕获后自动切换。删除当前屏幕后使用仍可捕获的屏幕。只修改保留策略不清除当前视频。

配置更新及设备变化失败时恢复原 XML、策略、设备启用状态和 CCD 拓扑；若恢复也失败，保留明确原因。不会接管其他应用拥有的驱动，不移除物理屏或最后一块可用屏。系统崩溃/断电后的临时屏恢复不在本次正常断开清理保证内。

数量上限未知，不能把它解释为任意数量必然可用；单次最多增加 32 屏，可分批请求。分辨率与刷新率还受随包签名 MttVDD 25.7.23、Windows、捕获及编解码器限制。固定驱动的 UINT32 中间乘积有已知限制：新增 4K120、8K120 模式可通过该检查，8K144、1080p59.94 会明确报告驱动计算限制；这不代表显卡的物理上限。状态和协议保留 1/10000 Hz 精度，未更改签名驱动或 Windows 安全策略。

回归面：`libs/base/protos/message.proto`、`libs/base/src/session_protocol.rs` 只增加版本化协议；`src/gpui_session.rs`、`controls.rs`、`src/client/io_loop.rs`、`src/ui_session_interface.rs` 只增加能力/请求/结果接线；`src/server/connection.rs` 与其 `media_controls.rs` 执行当前权限检查并确认结果；`src/virtual_display_manager.rs`、`mtt_vdd.rs`、`profile.rs` 接入真实库存、配置和正常退出清理；`crates/native-management/src/manager.rs`、`src/management/mod.rs`、`src/gpui_management.rs` 复用受保护的本地管理通道。桌面 `client.rs`、`client/media_controls.rs`、`client/toolbar_controls.rs`、`remote_video.rs`、`remote_video/media_controls.rs` 与移动 `session_chrome.rs`、`session_view.rs` 仅增加相应编辑器和结果处理。新实现位于各自的虚拟显示器子模块，旧主机保留原添加/移除入口，非 Windows 不显示本地驱动配置入口。


码率与显示后端基于 `53c4d39e4`，分段滑杆基于 `ecc1b68f2`；原生候选版本为 `0.1.0-beta.11`，Android versionCode 为 11。

## 操作与兼容

- 桌面会话侧栏和移动端显示面板提供 1–1000 Mbps 的码率上限滑杆及数值输入。四段 1–10、10–100、100–300、300–1000 Mbps 各占滑杆四分之一，拖动步长分别为 1、10、30、100 Mbps。300 Mbps 等分段端点始终可达，100–300 段为 100/130/160/190/220/250/280/300 Mbps。
- 拖动只更新本地预览，松手后提交一次；取消不提交。数值输入或键盘确认可设置到 0.001 Mbps 精度，输入未确认前不改变主机上限。实际生效值始终来自主机确认，解除上限保留原有自动适应。
- 十进制 1 Gbps 为 1000 Mbps、1000000 kbps。界面范围从 1 Mbps 起；公开协议入口保留旧客户端的 64 kbps 最小值兼容。
- 码率仍是编码目标上限。网络自适应、画质、帧率和共享流的其它观看者都可能让实际目标更低；它不是包含音频、协议开销和瞬时突发的逐包硬限速。恢复自动或断开会释放本连接上限。
- 主机明确上报 `max_bitrate_kbps`。旧主机只有原有支持标志时按其 100000 kbps 范围处理，超出主机范围的滑杆区域禁用且数值输入拒绝越界，固定分段比例不随主机上限重新均分；完全不支持调节的旧主机仍可观看和切屏。
- 分辨率和刷新率使用远端声明的完整模式。同分辨率下的不同 Hz 分别显示和确认。Windows 使用驱动原始模式及各旋转方向，不按固定常用分辨率或 60 Hz 截断。刷新率按 Windows DEVMODE 的整数 Hz 模式标识；自定义配置中的小数值按固定驱动支持的 1/10000 Hz 精度保留。
- Windows 先测试请求的模式，再应用并查询实际模式。只改刷新率、画面尺寸不变时，也发送模式确认。改变模式前释放原有输入，清除旧画面并请求新帧，首个新帧恢复画面与坐标输入。
- 旧主机没有刷新率字段时保留原有分辨率操作；非 Windows 主机继续使用已有分辨率实现。本轮不改视频几何、悬浮鼠标三键、触控坐标、横屏或登录框折叠逻辑。

## 虚拟显示器

Windows Host 新默认后端为 Virtual Display Driver 25.7.23。原始 AMD64 驱动 DLL 与 catalog 均由 SignPath Foundation 正式签名，固定归档大小为 132118 字节、SHA256 为 `e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a`。逐文件摘要、原始 MIT 许可来源和依赖记录见 `packaging/gpui-windows/virtual-display-vdd-driver.json`。旧 Amyuni 后端和其分发文件保留用于兼容，其自身四屏、十种模式等限制仍如实保留。

新配置合并实际显卡驱动报告的全部 RAW 模式与已有自定义模式，不将列表截断为十项、不固定为四个虚拟屏。模式不是根据显卡名称或制造日期推测。当前驱动没有可查询的可靠最大屏数，因此上报 `virtual_display_max_count: null`；每次添加均以 Windows 实际枚举结果确认，不能把未知最大值理解为任意数量必然可用。移除操作针对最近添加的虚拟屏，显示的计数 token 不是物理屏幕编号；保留最后一块显示器的原有规则。

安装 Host 只复制随包文件。显式添加虚拟屏，或已有的授权无头连接补屏路径，才按需执行 Windows 正常驱动安装。没有导入证书、开启测试签名或修改安全策略。初始化配置位于受安装目录保护的 `mtt_vdd/config`，保留既有设置；不接管其它应用管理的 VDD 设备或配置。配置原子写入，设备创建或模式重配置失败时恢复本次操作前的配置和设备状态，同时报告原始及恢复失败原因。升级/卸载保留配置和已注册共享驱动，只移除已知随包文件。

配置提交顺序为：首次创建所有权标记并同步文件、写临时 XML 并执行一次 `sync_all`、`MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` 原子替换、更新 VDDPATH、安装或重启设备、确认实际屏数。每次正常 XML 发布一次文件同步，首次标记另一次；恢复旧 XML 时再同步一次。注册表更新没有新增 `RegFlushKey`，普通禁用至零屏不重写 XML。

显示操作仍逐次检查当前管理授权和键鼠权限，驱动与系统调用在阻塞任务执行。系统、驱动和编码器拒绝请求时返回非致命错误，不把请求入队当作已生效。

## Intel 高码率

原固定 hwcodec 的 Direct-MFX 路径直接把 kbps 写入 16 位字段。现将同一固定源码及 SDK 头本地化到 `vendor/hwcodec`，仅为显式限制增加专用接口。使用 `BRCParamMultiplier` 表示高码率，保持有效目标不超过请求，避免 65535 以上回绕，并按比例换算缓冲，避免随倍率放大延迟。SDK Query、Reset 与实际参数查询必须成功，且不得悄悄改变 profile、level 或画面几何。恢复自动会恢复原有倍率和缓冲参数；未启用显式上限的原 setter 保持原实现。固定编码级别与具体 GPU/SDK 的真实限制继续有效，不将支持 1 Gbps 参数解释成每台硬件均能输出 1 Gbps。

## 必要回归面

| 既有文件 | 本轮改变的运行路径与必要性 |
| --- | --- |
| `libs/base/protos/message.proto` | 追加刷新率、当前模式及码率范围字段，保留所有旧字段编号和零值兼容语义。 |
| `libs/base/src/session_protocol.rs` | 外部码率上限放宽到 1000000 kbps，仍保留最小值及非法输入拒绝。 |
| `src/gpui_session/controls.rs` | 投影完整模式、主机码率范围和新驱动实际计数；请求保留 Hz，拒绝超出对端范围的码率。 |
| `src/gpui_session.rs` | 仅为既有显示回归测试补充刷新率零值，生产输入与媒体路径不变。 |
| `src/platform/windows.rs`、`src/platform/mod.rs` | 声明新的 Windows 模式模块，并为原生会话元数据提供薄入口；其它系统调用原实现。 |
| `src/server/connection.rs` | 登录时公告实际码率范围和完整显示模式。 |
| `src/server/connection/media_controls.rs` | Windows 模式应用及仅 Hz 变化的确认；虚拟屏按真实后端计数校验，保留原授权检查。 |
| `src/server/video_service.rs` | 显示变化时发送完整模式与当前 Hz，不改编码服务、帧队列或捕获几何。 |
| `src/server/video_qos.rs` | 只扩展既有测试至高码率数值，共享流和断连清理算法不变。 |
| `libs/scrap/src/common/vram.rs` | 显式限制走 MFX 专用安全倍率接口，反映 SDK 实际目标；普通自动路径不变。 |
| `libs/scrap/src/common/codec.rs` | 扩展现有真实 VP9/AV1 上限回归，覆盖 1 Gbps 参数。 |
| `libs/scrap/Cargo.toml`、根 `Cargo.toml` 与各受影响 `Cargo.lock` | 固定 hwcodec 改为仓库内相同 revision；Windows 配置解析使用已锁定 XML 库，启用实际 WinTrust API；没有无关依赖升级。 |
| `src/virtual_display_manager.rs` | 为 Windows Host 增加新后端薄路由，公告实际数量，旧后端实现保留。 |
| `src/server/display_service.rs` | 新驱动沿用既有按需无头补屏语义，普通无会话轮询不提前安装。 |
| `experiments/gpui-validation/src/client.rs` | 持有本地编辑控件，并在主机确认或能力变化时同步；不更改登录折叠或管理逻辑。 |
| `experiments/gpui-validation/src/client/media_controls.rs`、`remote_video/media_controls.rs` | 分段上限滑杆、精确输入、按对端范围启用、Hz 标签与确认、正确区分新旧驱动说明。 |
| `experiments/gpui-mobile-management/src/session_chrome.rs`、`session_view.rs` | 同样的分段上限滑杆、精确输入与模式投影/请求转发，不改变视频区域或输入门禁。 |
| `.github/scripts/package-gpui-windows.ps1`、`packaging/gpui-windows/host-installer.nsi` | 仅 Host 校验并分发新驱动，卸载不递归删除配置，不安装显示驱动。 |
| `packaging/gpui-windows/README-host.txt`、`THIRD_PARTY_NOTICES.txt`、`docs/gpui-windows-beta.md` | 说明新默认后端、原始许可、能力范围及按需安装行为。 |
| `.gitattributes` | 新许可固定 LF，确保 Windows checkout 后仍符合原始许可证摘要。 |
| `.github/workflows/gpui-windows-beta.yml`、`gpui-unix-beta.yml` | beta.11 版本与新依赖的触发范围，Windows 增加非空的实际模式配置回归。 |
| `packaging/gpui-android/app/build.gradle` | Android 版本升为 beta.11/code11，区分此前预览包。 |

新逻辑集中在 `src/platform/windows/display_modes.rs`、`src/virtual_display_manager/mtt_vdd/` 及其后端模块、驱动来源清单和独立准备脚本。码率滑杆、精确输入分别位于各前端功能子模块，分段比例、步进与十进制解析共用 `experiments/shared/bitrate_controls.rs`；没有新增持久化写入。hwcodec 原始来源、SDK 子 revision、保留文件清单与局部补丁记录随 vendored 源码提供。

## 验证边界

真实软件编码器回归、MFX 生产倍率 helper 的 SDK 编译断言、模式配置纯测试与 Windows 签名打包验证分别记录实际结果。完整 Windows、VRAM、macOS/Linux、Android/iOS 条件编译和应用回归由同一提交的原生 CI 验证。构建和单元测试不等同于物理 GPU、手机远控或全部显示模式的实测，不声称未经测量的吞吐量、帧率和延迟收益。最终可用模式还需满足捕获、编码和接收端渲染能力；本轮没有放松原有像素长度、溢出和渲染尺寸安全检查。


## 2026-11-02：退出清理、真实刷新率及移动端指针

Windows 虚拟屏设备的停用、重初始化和回滚之前，现在先申请独占的显示变更窗口。视频循环收到同一个生命周期标记后正常退栈，待编码器和捕获器实际析构，后台操作才继续；变更结束后，仍有订阅的视频服务按原有循环重建捕获。等待最多 5 秒，超时保留设备并返回错误。最后一个远程会话退出也使用这条释放确认；新连接登记与清理互斥，等待后重新检查管理授权，避免旧退出任务删除新会话的屏幕。不在连接列表或服务器锁下等待。远程配置的阻塞操作完成后再执行一次无会话检查，覆盖用户在配置期间退出、外层连接任务已取消而阻塞操作仍完成的情况；本机预配置不走这条补清理路径。

移除请求先记录期望数量 0；移除失败不再通过重新启用、重启适配器来回滚。自动无头登录不能覆盖已保存的 0 数量。固定签名 MttVDD 25.7.23 的 XML 模板要求正数量，因此 XML 的 1 不能代表实际仍有屏幕；实际状态使用 PnP/CCD。期望 0 而仍有设备时明确报告移除未完成。本次未改 CCD flags 或驱动安全策略；现场只读确认过虚拟屏的路径、信号及 GDI 当前模式均为 1920×1080@120Hz，物理屏为 60Hz，不能把视频 60FPS 直接当成虚拟屏 60Hz。

桌面和移动端的单一“刷新率”菜单按当前分辨率列出主机实际提供的 Hz，沿现有显示模式请求及实际确认路径设置。显示器选择使用下拉菜单。会话的采集目标跟随实际确认的 Hz，取消固定四档及 120FPS 的旧限制；传输安全上限为 1000FPS，以保留至少 1ms 的既有毫秒计时周期，这不是显卡或显示器能力上限，也不是实际帧率承诺。QoS、网络、解码与呈现反馈仍可降低帧率。

移动端鼠标模式绘制独立的 24 逻辑像素箭头，复用实际点击位置，不依赖远端光标是否隐藏、透明或尚无形状；边缘翻转箭头使其仍可见。触控模式保留原远端光标行为。远程画面外的全屏留空区域为纯黑。Android 会话申请当前物理尺寸下系统枚举的最高刷新率，退出或进入后台恢复原窗口首选模式及刷新率；系统节电、温度等策略仍可决定最终刷新率。

验证边界：本轮未使用 USB 调试，未执行 Android 实机验证。现场残留屏已通过正常 Windows 管理员确认，在服务及捕获进程全部退出后仅停用 XSCC 所有的 MTT 设备；物理屏保留，其他应用的虚拟驱动未变更。未对已报告卡死的机器重复执行创建/撤销压力测试，不能据源码或交叉编译宣称内核卡死已实测消除。新版本需要真实 Windows 原生编译、Android 包构建及安装后验收。

必要的既有运行路径变更：

| 文件/模块 | 变更原因及回归范围 |
| --- | --- |
| `src/server/connection.rs`、`connection/media_controls.rs`、`video_service.rs` 及其 `capture_lifecycle.rs` | Windows 连接登记、最后会话清理、捕获退出和恢复之间的释放屏障；非 Windows 保留原路径。 |
| `src/virtual_display_manager/mtt_vdd.rs`、`mtt_vdd/configuration.rs` | 所有已识别适配器变更入口持独占屏障；0 数量持久化、失败及自动无头行为修复；其他驱动不接管。 |
| `libs/base/src/session_protocol.rs`、`src/gpui_session.rs`、`src/client/io_loop.rs`、`src/client/streaming.rs`、`src/server/video_qos.rs`、`libs/scrap/src/common/codec.rs` | 原有 FPS 选项的合法范围及原生目标类型；沿用消息格式、QoS 和解码反馈；旧非流模式偏好范围保留。 |
| `experiments/gpui-validation/src/client.rs`、`client/toolbar_controls.rs`、`remote_video.rs`、`remote_video/media_controls.rs` | 真实 Hz、显示器下拉及实际模式确认后的目标同步；删除旧手动 FPS 入口。 |
| `experiments/gpui-mobile-management/src/remote_cursor.rs`、`session_view.rs`、`session_chrome.rs` | 可见本地指针、同坐标点击、真实 Hz/显示器菜单及纯黑留空；主页配色不变。 |
| `packaging/gpui-android/app/src/main/java/org/xscc/gpui/RemoteSessionPresentation.java`、`app/build.gradle` | 会话内临时高刷新率请求及恢复、beta20 包版本；签名、包名和绑定格式不变。 |
| Windows 工作流及既有 FPS/菜单测试 | 将生命周期释放和任意合法目标 FPS 回归纳入原生构建验证，避免空测试过滤通过。 |


## 2026-11-03：Hz 变更通知与不同屏幕的 FPS 隔离

同一虚拟屏只改变 Hz 时，宽高和捕获矩形可能不变，原几何检测不会发出显示模式变更。配置完成后，服务端现在依次发送新拓扑、在线当前屏的既有 `SwitchDisplay`（带即时读取的分辨率/实际 Hz）、操作结果。旧版原生客户端已支持这个消息，无需升级协议；只查询虚拟屏状态不会新增显示切换通知。

GPUI 被控端按当前视频服务真实订阅者计算采集 FPS 与编码器名义 FPS，避免另一块屏幕的 60FPS 会话把 120/240Hz 虚拟屏一同限制为 60。共享同一编码视频流的观看端仍采用其中最低的可持续 FPS；每个观看端的网络、解码反馈和首次连接保护继续生效。不新增缓存、线程或每屏策略状态，也不复制无关观看端数据。非 GPUI host 保留原全局计算路径。

新增既有运行路径范围：`src/server/service.rs` 只增加按需读取该服务订阅 ID 的方法；`video_qos.rs` 只增加借用对应用户数据的帧率投影；`video_service.rs` 在取屏周期、编码器创建与动态检查三处使用该投影。私有 `setup_encoder` 的两个相关调用改传视频服务，以取得真实订阅者；没有为无关调用增加占位参数。读取服务订阅 ID 后释放其锁，再读取 QoS，编码器变更仍在 QoS 锁外。`connection/media_controls.rs` 复用原消息通知实际模式；`gpui_session/controls.rs` 只增加兼容消息回归，没有客户端生产逻辑或包格式变更。

本地 Linux 验证：同屏同尺寸 60→120Hz 消息确认 1 项通过；GPUI host 实际 feature 下启动/QoS 回归 8 项通过，其中包括不同屏幕 60/240FPS 隔离和首次连接保护隔离两项。模拟/消息测试不等于实机 120FPS 验收。原生 Windows/macOS/Linux 的最终构建须包含此服务端修复；已验证 Android/iOS 包可继续使用原协议接收新通知。
