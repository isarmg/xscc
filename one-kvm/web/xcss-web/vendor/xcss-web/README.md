# xcss-web

面向浏览器的 Rust CSR 框架入口，同时提供共享组件、主题、国际化与管理 API 合同。
应用只依赖本 crate，通过 `xcss_web::prelude::*`、`xcss_web::html` 和
`xcss_web::component` 构建普通 Rust 函数式视图。后端继续使用 Axum。

内部运行时位于 `vendor/xcss-web-core`，过程宏位于 `vendor/xcss-web-macro`。
两者都以 CSR 为默认构建模式；产品构建校验禁止启用 SSR、hydration、full 与服务端函数依赖。

## 接入与函数式视图

独立产品将本目录的源码快照放入 `web/xcss-web/vendor/xcss-web`，在其前端 crate 中使用：

```toml
[dependencies]
xcss-web = { path = "vendor/xcss-web", default-features = false, features = ["csr"] }
```

```rust
use xcss_web::{ev, html, prelude::*};

#[component]
fn Counter() -> impl IntoView {
    let (count, set_count) = signal(0);
    html::button()
        .on(ev::click, move |_| set_count.update(|value| *value += 1))
        .child(move || count.get())
}

pub fn start() {
    xcss_web::mount("app", Counter);
}
```

所有组件、宏和运行时类型均通过同一个 `xcss_web` 入口访问，无需依赖内部 core 或 macro crate。

## 启动与失败恢复

应用入口导入本目录的 `startup.css`，并通过 `startup.js` 的 `startApplication` 加载 WASM：

```js
import "./vendor/xcss-web/startup.css";
import { startApplication } from "./vendor/xcss-web/startup.js";
await startApplication(async () => {
  const { default: initialize } = await import("./pkg/application.js");
  await initialize();
});
```

Rust `mount` 只在两种字重均加载、解码完成且视图成功挂载后报告就绪。
需要共享轮廓的 Server 页面使用 `mount_server`，它还会在报告就绪前启动 Rust 几何与 DOM 绘制控制器并完成首次绘制。控制器保留原生焦点、动态样式和卸载清理行为；产品入口不再导入 JavaScript 角标模块。
资源请求失败、WASM 无效、启动异常或 30 秒超时会显示可访问的错误和重新加载按钮，
不会自动刷新、循环重试或以系统字体渲染业务界面。失败提示自身使用系统字体，
即使应用字体不可用也能阅读；明确重载后重新开始启动。超时后的迟到结果不能覆盖错误页。

独立真实 WASM 浏览器验证：

```sh
node web/xcss-web/tests/startup-browser.mjs
```

需要 Rust 1.99.0、WASM target、wasm-bindgen 0.2.129 及已安装的 Chromium/Firefox；
`CARGO_TARGET_DIR` 与 `WASM_BINDGEN` 可指定本地缓存和固定工具。

静态 CSS、字体及许可证可从提交过的权威源码同步：

```sh
python3 scripts/vendor-xcss-web.py --component assets --destination /absolute/consumer/web/vendor/xcss
```

该命令在 xcss 仓库根目录执行；来源修订、完整文件清单和摘要写入 `SOURCE.json`，不导出退役的角标 JavaScript。

## 来源与许可证

框架实现基于 Leptos 0.8.19 裁剪并整合。原 MIT 许可证、版权通知及完整来源和修改记录
见两个内部 crate 的 `README.md` 与 `LICENSE`。共享组件和本仓库自有代码沿用仓库许可证。
`SOURCE.json` 为源码快照逐文件记录 SHA-256，包含这些 README 和许可证。
独立第三方依赖仍保留其真实 Cargo 包名和各自许可证。

## XSCS 内置副本的 G4 适配

此副本由 XSCS 仓库维护。`SOURCE.json` 的 `base_revision` 保留最初导入的
xcss 来源修订；当前实际源码由 XSCS 产品提交和逐文件 SHA-256 绑定。

`src/server_corners/profile.rs`、`geometry.rs` 及其几何回归来自 ZXS
`f009be2abbdcc56e5b7e8344461c97885a2e3f13` 的
`src/interface/src/contours/{profile,geometry}.rs` 与
`test/interface/unit/contours/geometry/tests.rs`。原项目：
https://github.com/isarmg/zxs 。采用其圆弧比例 0.4 的 `zxs-g4` 连续轮廓，
保留本仓库的绘制、CSS 属性与控制器生命周期，并增加可恢复的 profile 标识。
源码来源、许可和误差界还记录在 `src/server_corners/mod.rs` 中。
此适配不产生对 ZXS 或 xcss 外部仓库的构建、安装或运行依赖。
