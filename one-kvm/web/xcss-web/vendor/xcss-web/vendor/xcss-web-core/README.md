# xcss-web-core

`xcss-web` 的内部浏览器 CSR 运行时。产品只依赖公开入口 `xcss-web`。

## 来源与许可证

此代码基于 Leptos 0.8.19 的裁剪版本；沿用原 MIT 许可证，完整通知见同目录 `LICENSE`。以下保留原来源、修改记录和原文件摘要。`SOURCE.json` 验证当前源码与本 README 的 SHA-256。

```json xcss-upstream
{
  "experiment_round": 2,
  "fork_revision": "063311dfa46268a0fe5d4c9220a33a62e55e4491",
  "format": 1,
  "package": "leptos",
  "patch_sha256": "99d68359384b6216ae2f6cb27a615532566d985d235a6c8d923ffeb72c1af635",
  "previous_files": {
    "CSR_MINIMAL.md": "258ad8270a4f771512501000166c918b883d213630448922ada99ef55a7d0697",
    "Cargo.toml": "b63035cbd69746f65af9d98e63596d3612dd5d0384227e0223e2e69a72c91c72",
    "LICENSE": "76356e713a0d1ea0bde549f4236b01c750100ab31790019843faa5aec449310f",
    "Makefile.toml": "fdf0124f2b4d568fba61e5c40aab78f067f9ada5bcc9281446f87449c3ff2fae",
    "build.rs": "e212d639297796bd51e905411d3d9c77bf99de7e187769e420118df5d01f7cd4",
    "src/animated_show.rs": "fd008bfb13a8193427b78dc8e33dfaf1b2a17db3d29b3918e9daf20b0bcd98c3",
    "src/attribute_interceptor.rs": "0cef62a8ce7fc3c20d62354798b464e094792e753f8cdfe17443cdea135862ee",
    "src/await_.rs": "4b03542dff5ddc39efc13ddd10bd560122a79afd718340676955e0ecd19d246b",
    "src/children.rs": "060bbb5570704672df2125160c3190a3375f53e19905e9cdfb2a9e1a8be6daf6",
    "src/component.rs": "4b2ca289417e0075ad8ea42486daca256505125f6d8a73455ac663fd6f84cec4",
    "src/error_boundary.rs": "4c3d2e1aea0866d26b593e9603874f4f6a2dc78ccdfa0e39515c0a6853bc0a28",
    "src/for_loop.rs": "06be3281089fc8a330c99cf25f091ef3904dedfc1ca245e3d9d28b915ee3ac71",
    "src/form.rs": "65c19877cd71f51bc13aad275b5f3ecc34a2019b8dc9374091889d7b3003085f",
    "src/from_form_data.rs": "01ba4719c80b6fe911b091a7c05124b64eeece964e09c058ef8f9805daca546b",
    "src/hydration/hydration_script.js": "7f942d162a0ceb39dd354f6a46a3113a342ff5e4e7525451782a765ef8567409",
    "src/hydration/island_script.js": "ceb71195c4a8aad1f390f5798aa7163f8e976c49b6fc8414458f265b31e9293d",
    "src/hydration/islands_routing.js": "ad1cc8a78b18219a0d74be3fef38745f8bda87f585fde17a9b120d54fdfa9c1d",
    "src/hydration/mod.rs": "8b1416eb2d4e35179671a45d0ebd6ec52a44a6b8c9868ca195e48070b8956355",
    "src/hydration/reload_script.js": "fff584aebadcaa0cb164b1e814dcda5ecd844a72f50eb65c6b50c17a6cf1a165",
    "src/into_view.rs": "8ea18ae6c6c4a065163d91c45e761bec390a9345df0d756245d7ab0a31f23b55",
    "src/lib.rs": "b6b5d02adb3a73ab6cda84bf271363b550641ab4105ffdd34ee923fa871ca825",
    "src/logging.rs": "691a4243f0c89f8a9aabc053b3e116841447742282936d47ee59879e9cc2bbe8",
    "src/mount.rs": "e2dee4e0346dcd3088e4169a4e121e2440ce010012fd88dacd800e1a2a1b2327",
    "src/nonce.rs": "bde04a86d53bd2db0719f37084fc49b4fe1f930c69e0ba9b02a0db65bbaaad8f",
    "src/portal.rs": "235e4708a01cd95d5be7ebdaa50e9ea0f69bb3ac991b6ea1d848c78a2eb8649c",
    "src/provider.rs": "646aa92dd262172a844875bc94032ea10ace6a78b5ffda71efd8f4660960f95e",
    "src/show.rs": "5b7d4a40fd07e1bd97d325f19fb516bd950dc92805b133d54fabec201b5410ee",
    "src/show_let.rs": "a36e901a3bca3091986e75a7da49223d6682f724c29508cdb904622e3899e9f6",
    "src/subsecond.rs": "3b8c681841f2cf9d565a7ec07dc2ce37cd521daff1f71047a61eca9d3dcf9097",
    "src/suspense_component.rs": "e9ab1bae2ca4231805517c2b7197867889afc0f99a87169fc158a49a9758f924",
    "src/text_prop.rs": "1669d2085262467e3b30a1aa6302c9e18b8243b81443a5e3bfa5b0b76f5c6c30",
    "src/transition.rs": "0cafa7d6535945957a2c84abc2c6ad3842d790f0b3a11940f0f60df63821a6fc",
    "tests/pr_4061.rs": "7ef2e9f3cac08a09716aab67d056b01c5a6bb5d1edd49a91d2baf396e5f12e09",
    "tests/ssr.rs": "1012f74a525b425657466aacb98b14abea71c58926657bba398cedad603d5fa2"
  },
  "upstream": "https://github.com/leptos-rs/leptos",
  "upstream_revision": "fbc9ed631ff8c19c5eed02a1d4459e202cff7043",
  "version": "0.8.19",
  "xcss_rename": {
    "changes": [
      "Rename package, Rust paths and framework-facing runtime markers",
      "Expose the runtime and component macros through the xcss-web facade",
      "Keep MIT notices and previous source/patch metadata in this README"
    ],
    "internal_package": "xcss-web-core",
    "public_package": "xcss-web",
    "runtime": "csr"
  }
}
```

## 历史记录：CSR_MINIMAL.md

```text
# Leptos 0.8.19 CSR 裁剪实验

此分支基于官方 Leptos 提交 `fbc9ed631ff8c19c5eed02a1d4459e202cff7043`，只为现有 Axum REST API + Leptos CSR 应用提供可选的较小编译范围。它不是新的 UI 框架，也没有改变响应式图、Owner 清理或事件处理的实现。第二轮继续按功能开关拆分浏览器用不到的执行器与 Portal 的 Shadow DOM 支持。

## 使用方式

```toml
[dependencies]
leptos = { version = "=0.8.19", default-features = false, features = ["csr"] }

[patch.crates-io]
leptos = { path = "../leptos/leptos" }
leptos_macro = { path = "../leptos/csr_macro" }
```

路径应按应用目录调整。第二轮必须同时补丁 `leptos` 和配套的发布版 0.8.19 `leptos_macro`：新增的 `leptos_macro/full` 功能仅存在于 `csr_macro` 裁剪副本，官方发布版没有此功能，不能只替换 Leptos 主 crate。`csr_macro` 是独立包，不使用上游工作区中较早的宏版本。应用和共享组件库的 `leptos` 依赖都必须关闭默认功能；Cargo 会合并所有依赖边上的功能，任一依赖重新启用 `full` 都会恢复完整编译范围。

## 功能范围

- 默认启用新增的 `full` 功能，保留原始 0.8.19 的公开 API 和依赖功能范围。
- 关闭默认功能并启用 `csr` 后，保留 `view!` / `#[component]`、Signal、Memo、Effect、Owner、StoredValue、Callback、Context、Show、For、Portal、DOM 挂载、事件及异步任务等基础接口。
- 此最小模式不导出 `Resource` / `LocalResource` / `OnceResource` 等 `leptos_server` 资源类型、`ServerAction` / `ServerMultiAction`、服务端函数模块、`ActionForm` / `MultiActionForm`、`Await`、`Suspense` / `Transition`、`ErrorBoundary`、Leptos 服务端配置、`AutoReload` / `HydrationScripts` 等接口。当前试验的 xscs 与 xocs 使用自己的请求状态及错误反馈，不依赖这些接口。
- 通用 Action 类型仍来自 `reactive_graph`；它与被裁掉的 ServerAction 不同。xocs 的 `common::Action` 是应用自定义类型。
- 显式请求 `hydrate`、`hydration`、`ssr`、`islands` 或服务端序列化/传输功能时会自动启用 `full`。仅启用 `tracing` 不会重新引入 RPC 依赖。
- 第二轮将 `any_spawner/futures-executor` 移入 `full`，最小 CSR 仍启用 `any_spawner/wasm-bindgen`，挂载时仍通过 `Executor::init_wasm_bindgen()` 初始化浏览器执行器。其他依赖仍可能启用 `futures` 的默认 executor；此改动不等于移除所有 futures 执行器或异步任务支持。
- 第二轮新增 `shadow-dom` 功能，由 `full` 默认启用。最小 CSR 默认不编译 Portal 的 Shadow DOM 分支；仍保留 `use_shadow` prop，若未启用此功能却传入 `use_shadow=true`，会在挂载前明确 panic。需要 Shadow DOM 时可单独使用 `features = ["csr", "shadow-dom"]`，无需启用 `full`。
- 第二轮还关闭普通和 `cfg(erase_components)` 两条 `leptos_macro` 依赖边的默认功能，由顶层 `full` 显式恢复 `leptos_macro/full`，避免过程宏默认功能重新引入服务端函数宏依赖；组件擦除选项仍单独保留。

## 改动边界

`leptos/Cargo.toml` 以 crates.io 发布的 0.8.19 规范化清单为基础，保留发布版依赖版本范围，避免官方单一仓库中的路径继承把应用锁定的关联 crate 换成较早版本。应用原有 Cargo.lock 仍应保留并核对；本清单没有将所有关联 crate 固定到一个新的统一版本。

最小模式将 `leptos_config`、目标端 `leptos_hot_reload`、`leptos_server`、`server_fn`、`serde_qs` 和直接的 `hydration_context` 依赖变为可选，关闭 `tachys/reactive_stores`，并同步关闭依赖这些接口的模块及导出。这样可以取消由 `leptos_server` 传递启用的 `reactive_graph/hydration`，让 CSR Owner 不再携带供服务端渲染共享数据使用的上下文。

第一轮的 `Portal` 最小模式移除服务端共享上下文的浏览器判断，`full` 模式保留原判断。第二轮仅将创建 Shadow DOM 的分支及 Leptos 直接依赖中的 `web-sys/ShadowRoot`、`ShadowRootInit`、`ShadowRootMode` 绑定放到 `shadow-dom` 功能下；其他依赖仍可独立启用同名 web-sys 功能。两轮均保留 `wasm32` 目标判断、挂载位置、SVG 支持、Effect、视图卸载和 Owner 清理逻辑。`full` 或单独启用 `shadow-dom` 时的 Shadow DOM 分支保持原实现。

第一轮没有改动关联 crate 的实现。第二轮配套裁剪 `leptos_macro`，将服务端函数过程宏依赖设为可选，由 `leptos_macro/full` 恢复；构建主机上的 `leptos_hot_reload` 语法解析辅助函数仍保留。`tachys/testing` 也可能仍由未修改的 `leptos_dom` 启用，本实验不宣称关闭它。响应式图与 Tachys 的实现没有改动，SSR 与 hydration 的渲染 trait 在 Tachys 内仍然存在，并未全面删除。

减少源码编译范围不等于同比减少最终 WASM。需要分别比较依赖与功能图、干净构建时间、wasm-bindgen 产物及 gzip 大小，并运行现有真实浏览器回归。第二轮源码改动不能沿用第一轮结果作为验证；SSR、hydration、完整 API 的兼容性也需要对应的独立测试。测试结果由工作区实验报告记录；本文件只描述裁剪机制与兼容范围。

保留原 MIT 许可证及版权声明。

```

## 历史记录：original crate introduction

```text
//! # About Leptos
//!
//! Leptos is a full-stack framework for building web applications in Rust. You can use it to build
//! - single-page apps (SPAs) rendered entirely in the browser, using client-side routing and loading
//!   or mutating data via async requests to the server.
//! - multi-page apps (MPAs) rendered on the server, managing navigation, data, and mutations via
//!   web-standard `<a>` and `<form>` tags.
//! - progressively-enhanced single-page apps that are rendered on the server and then hydrated on the client,
//!   enhancing your `<a>` and `<form>` navigations and mutations seamlessly when WASM is available.
//!
//! And you can do all three of these **using the same Leptos code**.
//!
//! Take a look at the [Leptos Book](https://leptos-rs.github.io/leptos/) for a walkthrough of the framework.
//! Join us on our [Discord Channel](https://discord.gg/v38Eef6sWG) to see what the community is building.
//! Explore our [Examples](https://github.com/leptos-rs/leptos/tree/main/examples) to see Leptos in action.
//!
//! # Learning by Example
//!
//! If you want to see what Leptos is capable of, check out
//! the [examples](https://github.com/leptos-rs/leptos/tree/main/examples):
//!
//! - **[`counter`]** is the classic counter example, showing the basics of client-side rendering and reactive DOM updates.
//! - **[`counter_without_macros`]** adapts the counter example to use the builder pattern for the UI and avoids other macros,
//!   instead showing the code that Leptos generates.
//! - **[`counters`]** introduces parent-child communication via contexts, and the [`<For/>`](leptos::prelude::For) component
//!   for efficient keyed list updates.
//! - **[`error_boundary`]** shows how to use [`Result`] types to handle errors.
//! - **[`parent_child`]** shows four different ways a parent component can communicate with a child, including passing a closure,
//!   context, and more.
//! - **[`fetch`]** introduces [`Resource`](leptos::prelude::Resource)s, which allow you to integrate arbitrary `async` code like an
//!   HTTP request within your reactive code.
//! - **[`router`]** shows how to use Leptos’s nested router to enable client-side navigation and route-specific, reactive data loading.
//! - **[`slots`]** shows how to use slots on components.
//! - **[`spread`]** shows how the spread syntax can be used to spread data and/or event handlers onto elements.
//! - **[`counter_isomorphic`]** shows different methods of interaction with a stateful server, including server functions,
//!   server actions, forms, and server-sent events (SSE).
//! - **[`todomvc`]** shows the basics of building an isomorphic web app. Both the server and the client import the same app code.
//!   The server renders the app directly to an HTML string, and the client hydrates that HTML to make it interactive.
//!   You might also want to see how we use [`Effect::new`](leptos::prelude::Effect) to
//!   [serialize JSON to `localStorage`](https://github.com/leptos-rs/leptos/blob/20af4928b2fffe017408d3f4e7330db22cf68277/examples/todomvc/src/lib.rs#L191-L209)
//!   and [reactively call DOM methods](https://github.com/leptos-rs/leptos/blob/16f084a71268ac325fbc4a5e50c260df185eadb6/examples/todomvc/src/lib.rs#L292-L296)
//!   on [references to elements](https://github.com/leptos-rs/leptos/blob/20af4928b2fffe017408d3f4e7330db22cf68277/examples/todomvc/src/lib.rs#L228).
//! - **[`hackernews`]** and **[`hackernews_axum`]** integrate calls to a real external REST API, routing, server-side rendering and
//!   hydration to create a fully-functional application that works as intended even before WASM has loaded and begun to run.
//! - **[`todo_app_sqlite`]** and **[`todo_app_sqlite_axum`]** show how to build a full-stack app using server functions and
//!   database connections.
//! - **[`tailwind`]** shows how to integrate TailwindCSS with `trunk` for CSR.
//!
//! [`counter`]: https://github.com/leptos-rs/leptos/tree/main/examples/counter
//! [`counter_without_macros`]: https://github.com/leptos-rs/leptos/tree/main/examples/counter_without_macros
//! [`counters`]: https://github.com/leptos-rs/leptos/tree/main/examples/counters
//! [`error_boundary`]: https://github.com/leptos-rs/leptos/tree/main/examples/error_boundary
//! [`parent_child`]: https://github.com/leptos-rs/leptos/tree/main/examples/parent_child
//! [`fetch`]: https://github.com/leptos-rs/leptos/tree/main/examples/fetch
//! [`router`]: https://github.com/leptos-rs/leptos/tree/main/examples/router
//! [`slots`]: https://github.com/leptos-rs/leptos/tree/main/examples/slots
//! [`spread`]: https://github.com/leptos-rs/leptos/tree/main/examples/spread
//! [`counter_isomorphic`]: https://github.com/leptos-rs/leptos/tree/main/examples/counter_isomorphic
//! [`todomvc`]: https://github.com/leptos-rs/leptos/tree/main/examples/todomvc
//! [`hackernews`]: https://github.com/leptos-rs/leptos/tree/main/examples/hackernews
//! [`hackernews_axum`]: https://github.com/leptos-rs/leptos/tree/main/examples/hackernews_axum
//! [`todo_app_sqlite`]: https://github.com/leptos-rs/leptos/tree/main/examples/todo_app_sqlite
//! [`todo_app_sqlite_axum`]: https://github.com/leptos-rs/leptos/tree/main/examples/todo_app_sqlite_axum
//! [`tailwind`]: https://github.com/leptos-rs/leptos/tree/main/examples/tailwind_csr
//!
//! Details on how to run each example can be found in its README.
//!
//! # Quick Links
//!
//! Here are links to the most important sections of the docs:
//! - **Reactivity**: the [`reactive_graph`] overview, and more details in
//!   + signals: [`signal`](leptos::prelude::signal), [`ReadSignal`](leptos::prelude::ReadSignal),
//!     [`WriteSignal`](leptos::prelude::WriteSignal) and [`RwSignal`](leptos::prelude::RwSignal).
//!   + computations: [`Memo`](leptos::prelude::Memo).
//!   + `async` interop: [`Resource`](leptos::prelude::Resource) for loading data using `async` functions
//!     and [`Action`](leptos::prelude::Action) to mutate data or imperatively call `async` functions.
//!   + reactions: [`Effect`](leptos::prelude::Effect) and [`RenderEffect`](leptos::prelude::RenderEffect).
//! - **Templating/Views**: the [`view`] macro and [`IntoView`] trait.
//! - **Routing**: the [`leptos_router`](https://docs.rs/leptos_router/latest/leptos_router/) crate
//! - **Server Functions**: the [`server`](macro@leptos::prelude::server) macro and [`ServerAction`](leptos::prelude::ServerAction).
//!
//! # Feature Flags
//!
//! - **`nightly`**: On `nightly` Rust, enables the function-call syntax for signal getters and setters.
//!   Also enables some experimental optimizations that improve the handling of static strings and
//!   the performance of the `template! {}` macro.
//! - **`csr`** Client-side rendering: Generate DOM nodes in the browser.
//! - **`ssr`** Server-side rendering: Generate an HTML string (typically on the server).
//! - **`islands`** Activates “islands mode,” in which components are not made interactive on the
//!   client unless they use the `#[island]` macro.
//! - **`hydrate`** Hydration: use this to add interactivity to an SSRed Leptos app.
//! - **`nonce`** Adds support for nonces to be added as part of a Content Security Policy.
//! - **`rkyv`** In SSR/hydrate mode, enables using [`rkyv`](https://docs.rs/rkyv/latest/rkyv/) to serialize resources.
//! - **`tracing`** Adds support for [`tracing`](https://docs.rs/tracing/latest/tracing/).
//! - **`trace-component-props`** Adds `tracing` support for component props.
//! - **`delegation`** Uses event delegation rather than the browser’s native event handling
//!   system. (This improves the performance of creating large numbers of elements simultaneously,
//!   in exchange for occasional edge cases in which events behave differently from native browser
//!   events.)
//! - **`rustls`** Use `rustls` for server functions.
//!
//! **Important Note:** You must enable one of `csr`, `hydrate`, or `ssr` to tell Leptos
//! which mode your app is operating in. You should only enable one of these per build target,
//! i.e., you should not have both `hydrate` and `ssr` enabled for your server binary, only `ssr`.
//!
//! # A Simple Counter
//!
//! ```rust
//! use leptos::prelude::*;
//!
//! #[component]
//! pub fn SimpleCounter(initial_value: i32) -> impl IntoView {
//!     // create a reactive signal with the initial value
//!     let (value, set_value) = signal(initial_value);
//!
//!     // create event handlers for our buttons
//!     // note that `value` and `set_value` are `Copy`, so it's super easy to move them into closures
//!     let clear = move |_| set_value.set(0);
//!     let decrement = move |_| *set_value.write() -= 1;
//!     let increment = move |_| *set_value.write() += 1;
//!
//!     view! {
//!         <div>
//!             <button on:click=clear>"Clear"</button>
//!             <button on:click=decrement>"-1"</button>
//!             <span>"Value: " {value} "!"</span>
//!             <button on:click=increment>"+1"</button>
//!         </div>
//!     }
//! }
//! ```
//!
//! Leptos is easy to use with [Trunk](https://trunkrs.dev/) (or with a simple wasm-bindgen setup):
//!
//! ```rust
//! use leptos::{mount::mount_to_body, prelude::*};
//!
//! #[component]
//! fn SimpleCounter(initial_value: i32) -> impl IntoView {
//!     // ...
//!     # _ = initial_value;
//! }
//!
//! pub fn main() {
//! # if false { // can't run in doctest
//!     mount_to_body(|| view! { <SimpleCounter initial_value=3 /> })
//! # }
//! }
//! ```


```
