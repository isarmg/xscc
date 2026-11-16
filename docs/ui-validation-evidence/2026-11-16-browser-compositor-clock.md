# 2026-11-16 浏览器 UFO 60Hz 对照

用户要求始终保持物理屏为主屏。复用现有虚拟屏，未新建、撤销或切换主屏，未进入浏览器全屏。物理屏仍为 1920×1080/60Hz，虚拟屏为 1920×1080/120Hz。Chrome 157.0.8103.52；Windows 11 26H2，build 26300.9974。

## 源码与只读核对

Chromium [DXGI 同步实现](https://raw.githubusercontent.com/chromium/chromium/main/ui/gl/vsync_thread_win_dxgi.cc) 选择主显示器并等待其 WaitForVBlank；[实现选择入口](https://raw.githubusercontent.com/chromium/chromium/main/ui/gl/vsync_thread_win.cc) 依据 UseCompositorClockVSyncInterval 选择 DXGI 或 DComp 同步路径。[特性定义](https://raw.githubusercontent.com/chromium/chromium/main/ui/gl/gl_features.cc) 默认关闭合成时钟路径，且要求 Windows 11 24H2 或更新系统。核对的是官方 main 源码，未声称它与本机 157 二进制逐字节一致；本机 DLL 确认含该特性名。

在用户原 UFO 页面保持原样时，系统目标统计显示虚拟输出 vblank 120.00048Hz、refresh 120Hz，呈现活动约 60Hz。它证明模式/输出时钟为 120Hz，不能证明浏览器每秒提供 120 个不同图像。

## 用户授权的独立窗口对照

用户此前限制浏览器操作，本次明确同意独立临时 UFO 窗口。使用同一隔离配置、相同浏览器、相同页面和显示器，仅切换官方同步特性：

| 选项 | UFO 刷新率数值 |
| --- | ---: |
| 明确关闭 | 59.992Hz |
| 开启，第一次 | 120.001Hz |
| 再开启 | 119.997Hz |

没有禁用垂直同步、取消帧率上限、伪造 vsync 间隔或改写网页计数。两组均有 UFO 同步失败警告，不能将大字数值写成稳定性校验通过。开启后一次系统目标呈现活动约 119.07Hz，仍不替代独立应用像素测量。

15 秒本地 Chrome 诊断包含 510,198 事件。合成时钟参数回调 1,728 次，首尾覆盖 15.466 秒，平均约 111.66Hz；间隔中位数 8.337ms、p95 16.599ms，125 个间隔超过 12ms。仅一次回退 Sleep，约 9.398ms，不能把持续抖动归为频繁的回退 Sleep。

另用原生诊断线程直接等待同一 Windows 合成时钟 8 秒，898 个间隔，零失败；平均 112.37Hz，中位数 8.377ms、p95 16.435ms，60 个间隔超过 12ms。说明延长间隔也能在独立调用该系统接口时观察到；尚不能仅据此区分 Windows 调度、驱动与组合负载。没有针对猜测原因修改驱动、拥塞控制或全局 CPU 设置。

## 本机应用与限制

用户随后明确同意创建桌面“Chrome 高刷”入口并重启现有 Chrome。已正常退出旧 Chrome，以同一用户配置加上 UseCompositorClockVSyncInterval 与 restore-last-session 启动，原 UFO 标签页恢复。恢复后小窗口已还原为虚拟屏内的普通最大化窗口。隔离测试窗口已关闭，原有浏览器配置、原快捷方式未改；新入口保留该启动参数。正常启动而未带该参数时，旧路径仍可能恢复 60FPS。

重启后的原 UFO 页面约 111FPS，60 的硬上限已突破；仍出现 Browser unable to VSYNC，同步校验尚未通过，不能宣称全程稳定 120FPS。远控已暂时断开用于隔离编码负载，虚拟屏继续保留。
