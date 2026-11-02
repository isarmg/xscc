# XSCC / One-KVM 五轮排查记录

日期：2026-11-02。覆盖桌面与移动控制端、原生被控端和 One-KVM。按固定五轮执行，兼顾稳定性、输入延迟、帧率/画质、CPU、内存和并发。每项改动独立提交并推送 main；下列结果仅对应明确记录的源码和执行环境。

## 基线

- 并仓基线：`01b5c782e30f34931747d506ae024ec47dbfe061`，已推送 `isarmg/xscc/main`。
- One-KVM 本地完整宿主：585 项通过；此前真实 CLI/HTTP 30 项通过。
- 本轮原生软件核心验证：`server::video_` 52 项、`gpui_session::tests` 20 项、呈现 3 项、拥塞恢复 3 项通过。使用真实 Linux 软件核心，未以此代替 Windows 硬编或实机验证。
- 多平台 CI 按提交 SHA 查询；尚在运行的矩阵不计为通过。当前环境的 WebRTC ICE 和 Firefox WebAudio 限制沿用原验证记录。

## 第一轮：正确性与生命周期

### 慢 WebSocket 背压与撤权

真实 WebSocket codec 与 64 字节 duplex 停读可使 1MiB 发送进入 Pending；原先进入发送分支后，外层撤权分支不再被 poll。原事件转发也使用无界队列。

修复将事件队列限制为 256（与现有事件总线窗口一致），生产任务等待有限容量；单次发送同时等待授权取消和 5 秒 deadline。受阻客户端退出仍执行原事件任务 abort 与 HID 清理，音频、UAC、HID 的状态/心跳/关闭发送使用相同约束。

验证：8 项 WebSocket 测试通过，其中 3 项新增回归分别覆盖受阻发送取消、有限 deadline 和队列背压/断开释放。普通 Clippy、格式检查通过，保留既有宿主告警。改善的是队列内存上界与退出响应，不把它宣称为物理视频帧率提升。

必要回归面：`one-kvm/src/web/ws.rs` 的事件转发与发送；`audio_ws.rs`、`uac_ws.rs` 和 `hid/websocket.rs` 的受阻发送退出；`web/mod.rs` 仅导出同一局部 helper。

### 当前显示器离线后的画面与输入

拓扑广播与视频服务的切屏通知来自独立路径；移除当前屏后不保证马上收到 SwitchDisplay。桌面现在清除旧画面和已按键鼠，离线时拒绝新按键、移动和滚轮，仍允许释放事件；恢复在线后清掉本次离线提示并恢复接收新帧。on_rgba在现有锁内按索引检查可用性，拒绝离线迟到帧，没有新状态或每帧列表扫描。

新增真实SessionHandle回归在迟到帧场景先失败，修复后会话20项及软件核心完整203项通过。Windows控制端CI扩大为全部会话回归，保留非零测试匹配检查。必要回归面仅桌面RemoteVideo、共享会话API的输入/帧接纳边界及对应Windows门禁；IO、解码和协议格式没有因此重写。

### 设备配置被取消时的一致性

真实 ConfigStore、共享 ResourceRegistry 和 SQLite 池复现：运行时已切到 camera-b，保存配置阻塞时取消 HTTP 请求，原实现会释放 camera-b 的候选占用，另一实例可能分配仍在使用的设备。

已接纳的配置事务现在在现有 Tokio 运行时中独立完成提交或回滚，并持有原有领域锁；并发更新仍立即报 busy，不新增等待队列。拿锁后、启动事务前同步重验现有 AccessGrant，避免设备枚举期间撤权的旧请求重新接纳。回滚失败会尝试禁用设备并保留候选占用，要求停止该实例后再配置；不会把可能仍活动的设备交给其他实例。实例停止先撤权和关闭媒体/原生会话，再等待这些更新，最后清理硬件。

验证：取消后占用及领域锁保留、落盘后释放旧设备、回滚失败占用隔离两项回归通过；One-KVM 完整宿主库 590 项通过。复现使用真实数据库及资源注册表，硬件应用状态以测试局部 Mutex 表达，未声称实际设备验证。

必要回归面：配置 video/audio/ATX/native/USB 更新和 setup；USB 各入口及 audio/device 仅传递已有授权；资源租约增加失败清理后的保留占用；运行时停止顺序等待已接纳事务。未修改协议或持久化格式。内部任务 panic 和运行时强制终止不在本次取消一致性保证内；独立的流模式切换任务留给第四轮审查。

WebSocket修复提交：`348c04f47d3bb73a9c565acfd8fb9e8e1e79b68c`。

## 第二轮：控制端输入、接收与解码

真实 loopback TCP 停读探针连续 5 次均让实际 Stream::Tcp 发送进入 Pending；10000 个输入与 Close 留在 FIFO，已到期的检查不能进入。实际默认发送 timeout 是 18000ms。原 UI 分支忽略错误、返回 true，还会继续发送下一条，在不可复用的半帧/加密流上重复等待。

最小修复让 UI 消息、初始剪贴板、提权和截图这五处发送失败进入已有错误通知并退出原 IO 循环，随后由原断连路径清理。真实 Remote 的新增回归明确等待发送 Pending、超时后返回 false，释放和 Close 保留原序，Remote 销毁后 sender 关闭；仅恢复 Data::Message 原调用点时回归失败，恢复补丁后通过。真实 loopback 对照 5 次均由“错误后尝试两次、循环未退出”变成“尝试一次、退出”。这验证错误后的终止行为，不是物理输入延迟基准。

必要回归面只有 `src/client/io_loop.rs` 五个 UI 发送调用点及局部错误处理；Windows 控制端增加非空测试 filter，Unix 工作流已有完整核心门禁。登录、Close、协议、cipher、channel 类型和视频队列未重写。首次发送仍可阻塞到原有 18 秒 deadline；输入 FIFO 仍无容量上限，属于量化保留限制，不能宣称已全面解决慢链路输入背压。

现有 120 帧 delta 队列的参考链失效/关键帧恢复测试连续 5 次均 3/3 通过；硬溢出恢复不受周期软刷新 20 次限制。保留队列和参考帧解码，不用任意丢帧换性能。关键帧通知绕过 delta 队列是只读候选，未用真实慢解码负载复现，列为待实测限制。

## 第三轮：原生被控端 QoS 与热路径

两个不相交视频服务 A/B 的用户分别设为 8000/500 时，原全局 hard-cap 查询使 A 也得到 500；驱动拒绝 A 的 cap 时，原清理还会清掉 B 的 cap。修复仅让 GPUI host 在编码器初始化和逐帧检查时按当前流的订阅者取最小 cap，拒绝后也只清该流的 cap。ratio、latest_quality、VBR 保持原有全局 ABR 语义，非 GPUI 路径保留原行为。

两项新增回归修前失败、修后通过，真实软件核心 `server::video_` 54 项通过。必要回归面只有 `src/server/video_qos.rs` 的按订阅者查询/清理和 `src/server/video_service.rs` 的三个调用位置；不新增状态、嵌套锁或缓存。Windows 已有 `server::video_` 非空测试门禁，Unix 完整原生核心测试覆盖相同用例。

只读性能证据：同样 4 个 active、1 个 pending 订阅者，100000 次查询 ×5，原订阅者快照均分配 100000 次/2000000 字节。消除它需要缓存或改变锁嵌套，收益未验证，本轮保留。真实 pipeline 在持续 1000 条正常 CursorPosition 控制消息的饱和探针中，5 次均先发完控制消息才发首视频；常态低频负载没有复现该问题。保留优先级，避免未经验证改变控制/原始数据顺序。并行编译期间未把墙钟耗时宣称为性能提升。

## 第四轮：One-KVM 多实例与并发

真实 VideoStreamManager 复现：切换请求先把 switching 设为 true，再等待 transition ID 锁；此时请求被取消会永久返回 busy。现在先完成可取消的等待，CAS 接纳后无 await 地启动原有异步任务，开始事件由任务发布。模式任务复用已有 codec_change 互斥锁，stop/clear_video_source 等待已接纳切换完成后再清理，避免旧任务在最终停止后重开视频。

两项真实管理器回归均修前失败、修后通过；该模块 5 项通过。必要回归面只有 `one-kvm/src/video/stream_manager.rs` 的模式接纳、原有 codec/native admission 的串行边界以及 stop/clear 的等待。没有新增任务表、flag 或设备缓存；独立调用链审查未发现递归获取同锁。其他视频启动入口未在本修复中形成统一关闭屏障，不能宣称整个视频生命周期已完全封锁。

实例路由已经禁止宿主 terminal/extensions，但 setup 的旧 ttyd_enabled 字段仍允许启动宿主级 shell 扩展进程，绕过同一能力边界。现在在 JSON 边界拒绝 true，保留 false/缺省/null 兼容，并删除 setup 自动启动 ttyd 的路径。新增请求回归在原解析行为下失败、修复后通过。必要回归面只有 `web/handlers/setup.rs`；不声称该问题本身已证明可从外部访问宿主 shell。

第四轮最终宿主快照：593 项完整 all-targets 测试通过，格式检查和普通 Clippy 通过（保留原有 56/85 项告警）。独立复审配置收尾、占用计数、锁序、模式切换和 setup 边界，未发现新增阻塞问题。

跨实例只读复查覆盖 cookie 名称/路径重写、其他实例和 legacy cookie 丢弃、catalog 软链接拒绝、自动设备通配占用、Windows 全局桌面键鼠独占，未找到新的可复现跨实例越权。Windows 每帧显示器枚举尚无实机量测，未据此添加拓扑缓存。

## 第五轮：独立复审与组合回归

独立复审覆盖前四轮新增的队列/发送退出、领域锁与占用计数、失败回滚、关闭顺序、输入清理、QoS feature 边界、模式切换以及 setup 能力边界。对最终 diff 做最小化检查，未引入新的协议格式、数据库迁移、依赖、缓存或测试生产钩子。

| 本地验证 | 最终结果 | 适用范围 |
| --- | --- | --- |
| One-KVM `cargo test --locked --all-targets` | 593 通过 | Linux 宿主逻辑，含真实 SQLite、TCP/WebSocket |
| XSCC 软件核心完整库测试 | 206 通过 | Linux，`use_dasp,gpui-host,linux-pkg-config` |
| 新构建 One-KVM CLI/HTTP | 30 通过 | 临时三实例、独立数据库、真实嵌入 WASM、拒绝未授权 API、退出清理 |
| 组合重复回归 | 5 轮 ×15，75 次全部通过 | WS、模式切换、事务取消、清理失败占用隔离；20 次调用均非空匹配 |
| Enigo 错误格式化回归 | 1 通过 | 真实 tokenize 错误可输出原有描述，独立子进程不再栈溢出 |

最终 lint 扩查复现 Enigo 公开 DSL 的 ParseError 格式化递归：受控子进程格式化失败后栈溢出退出（-6）；修复后正常退出。只把 Display 中递归的 to_string 换为既有 description，保留文案、接口和其余行为。新增一项 tokenize→Display 回归和普通包 Clippy 通过；必要回归面只有 `libs/enigo/src/dsl.rs` 的公开错误格式化。没有发现当前产品调用该可选 DSL，未将它宣称为远程键盘可触发漏洞。

One-KVM 普通 Clippy 和格式检查通过。GTK sudo 参数正则无效的执行分发仅在保留 Flutter/core_main 入口，当前 GPUI background command 不支持该命令，未将其当作当前产品可达漏洞，亦未改动历史路径；它仍阻塞全库的 `clippy::invalid_regex` 检查。最终 root `--no-deps` Clippy 只放行这一旧检查后通过，不能据此宣称标准全库 Clippy 全绿。最终 Enigo 修复后重新构建并运行的 206 项软件核心测试通过。

### 提交记录

| 改动 | 已发布提交 |
| --- | --- |
| 上一轮并仓及裸机/Windows/多实例 | `01b5c782e30f34931747d506ae024ec47dbfe061` |
| WebSocket 背压及撤权 | `348c04f47d3bb73a9c565acfd8fb9e8e1e79b68c` |
| 当前显示器离线清理 | `88dabfd7508a3f08fb816984ac677536d640e987` |
| 配置事务取消一致性 | `d7882f072788dc5892782de98a5df364c94a9c0c` |
| 每流码率上限 | `cd912f9104c25da7fc10f2627f554e9d7bb64c11` |
| 模式切换接纳及停止等待 | `e8664fc7675ae4e3304ec09d41d7a3ed151c1a32` |
| UI 发送错误后断连 | `fbeb9de928027513caa7e92b1507aa8676ebc522` |
| setup 禁止宿主终端扩展 | `16ebd19cf4833e104afed7f7f4b0c11a38b4d6e0` |
| Enigo 公开错误格式化 | `cc9667071f53640a5c1f95d13f24c2df9db87f04` |

Git CLI 临时认证失效后改用现有 GitHub 连接器的 Git Data API 发布，逐 blob、整棵树及本地重建 commit 均核对 SHA，再以预期父提交推进 main，未强推。上表使用 GitHub 上的实际提交 SHA。

### 验证限制

多平台 CI 按各源码提交持续查询；被新提交取消的任务、排队、运行中和未触发的矩阵均不计为通过。初始并仓的 Windows/Unix 原生矩阵已完成，不能据此替代最终源码的平台验证。没有创建 tag、发布 release 或部署。

最终查询快照：2026-11-02 19:13:44 UTC。

| 源码 | CI 实际状态 |
| --- | --- |
| 最终原生核心 `cc966707` | 源码检查通过；framework/management 的部分 job 正运行，其余桌面/移动矩阵运行或排队，尚未全矩阵通过 |
| 最终 One-KVM `16ebd19` | [run 41507212775](https://github.com/isarmg/xscc/actions/runs/41507212775) 正构建浏览器，fmt/contracts 已过；Linux/Windows 原生矩阵尚未开始，尚未全矩阵通过 |
| 历史完整通过证据 | 并仓 `01b5c78` 的七项非 One 工作流；Android/iOS `88dabfd`；framework `cd912f9`；Native management `e8664fc`。各证据仅对应其源码 |

此快照未观察到本轮 CI 失败；该结论不能推导为未完成的任务将通过。最终报告文档提交不改变产品源码。

当前环境无物理显示器、摄像头、音频和 GPU 测试设备；Windows/MSVC、macOS、Android/iOS 最终运行验收仍依赖对应 CI 和实机。WebRTC ICE、浏览器 WebAudio 环境限制沿用原集成记录。没有物理端到端延迟、FPS、CPU/GPU 提升百分比结论。

每轮保留未复现候选和验证限制。没有超过基线波动的收益，不引入对应性能改动；不通过扩大无界缓存、任意丢弃参考帧或放宽授权换取数字。
