# Rust 原生 Web 构建工具

这个独立 Cargo workspace 为现有 xcss-web CSR 项目生成 Axum 可嵌入的静态资源。生产构建只执行锁定的 Cargo 和 wasm-bindgen；不执行 Node、Vite、Python，不下载 CDN 运行时。Node/Playwright 保留为真实 WASM 浏览器测试工具；共享 Web 资源包不包含旧界面和构建运行时。

```sh
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- build --product-root . --offline
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- check --product-root . --offline
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- test --product-root . --offline
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- serve --product-root . --debug --offline
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- preview --product-root . --bind 127.0.0.1:0
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- server --product-root . --mode development --offline
```

首次准备需要 Rust `1.99.0`、`wasm32-unknown-unknown` target、wasm-bindgen CLI `0.2.129`，以及两个独立 Cargo.lock 的缓存依赖。`--offline` 控制浏览器 Cargo，工具自身 Cargo 也应加 `--offline`。`WASM_BINDGEN` 可指定已验证版本的 CLI。工具与浏览器分开使用 target：`CARGO_TARGET_DIR` 控制启动工具和原生服务器，`XCSS_WASM_TARGET_DIR` 控制浏览器编译，默认 `<product>/target/xcss-web`。浏览器默认两个编译 job；显式 `CARGO_BUILD_JOBS` 可覆盖。

构建工具的唯一维护来源为 xcss 的 `tools/web-build`。在所选 xcss 源码根目录，将已提交的工具源码、锁文件、测试、说明及原始许可证分发到明确的产品目录：

```sh
python3 scripts/vendor-xcss-web.py --component builder --destination /absolute/product/tools/web-build
python3 scripts/vendor-xcss-web.py --component builder --check --destination /absolute/product/tools/web-build
```

`SOURCE.json` 绑定完整 Git 修订、来源目录和逐文件 SHA-256。声明嵌入式 Web 构建的产品必须保留该清单；构建前后核对来源、完整库存和实际编译输入，并将工具来源写入资源 provenance。下游不手改快照。没有清单的已有目录仅在所有既有文件与所选已提交源码逐字节一致时接管；改过或未知文件会阻断覆盖。已有物理 `target/` 缓存不分发、不计入源码库存，也不由同步删除。

## 配置

读取产品的 `web/rust-web.toml`。未知配置字段与未知 CLI 选项会报错。普通 CSR 示例：

```toml
[web]
kind = "spa"
entry = "src/main.js"
base = "/"
dist = "web/dist"
licenses = [
  "vendor/xcss/web-fonts/OFL.txt",
  "vendor/xcss/web-fonts/CJK-LICENSE.txt",
  "vendor/xcss/web-fonts/NORMAL-LICENSE.txt",
]

[web.proxies]
"/api" = "http://127.0.0.1:18104"

[aliases]
"@xcss/web/" = "vendor/xcss/"
# 精确入口也受支持。成熟 HLS 库必须保留其实现与许可：
# "hls.js" = "vendor/hls/hls.mjs"
```

未写 `entry` 时优先 `src/main-xcss-web.js`，其次 `src/main.js`。`base` 只接受同源 `/` 或 `/prefix/`。`--dist` 或 `XCSS_WEB_DIST` 覆盖输出；默认按产品根目录解释相对路径。输出不得覆盖源目录或源目录祖先，现有非空输出必须含 xcss-web 生成清单，否则拒绝替换。

`xczs` 保留服务器生成 HTML 和 `__ASSETS_PREFIX__`、`__INDEX_DATA__`、`__WASM_PRELOAD__` 模板。工具不把它转换成 SPA：

```toml
[web]
kind = "embedded"
dist = "web/runtime-dist"
relative_urls = true
runtime_root = "product"
css_output = "dist/platform.css"

[web.entries]
"index.js" = "xcss-web-index.js"
"login.js" = "xcss-web-login.js"
"dist/platform.js" = "xcss-web-platform.js"

[web.copies]
"index.css" = "index.css"
"login.css" = "login.css"
"favicon.ico" = "favicon.ico"
"dist/boot.css" = "vendor/xcss/web-fonts/boot.css"
"dist/MapleMonoFull-Regular.woff2" = "vendor/xcss/web-fonts/MapleMonoFull-Regular.woff2"
"dist/MapleMonoFull-Bold.woff2" = "vendor/xcss/web-fonts/MapleMonoFull-Bold.woff2"
```

多入口输出只增加很小的 ESM facade，实际模块以内容 SHA-256 的前 16 位命名。公共模块只输出一次。字体固定 URL 与历史模板兼容，字体数据不会再额外复制一份哈希文件。embedded CSS 内作者提供的 SVG data URL 转为同源 SVG 文件，保持禁用 `data:` 的原 CSP。

## 资源与追溯合同

- Rust 浏览器编译保持 `--locked`；正式构建默认 release，`--debug` 使用 dev profile。精确校验 wasm-bindgen CLI 版本。
- 编译前检查 `cargo metadata --locked --filter-platform wasm32-unknown-unknown` 的实际依赖图：xcss-web 核心和宏必须各有唯一版本，且来自 xcss-web 集成目录。公开 `xcss-web` facade 必须唯一且指向项目内集成源码；内部 `xcss-web-core` 和 `xcss-web-macro` 的目录与 Cargo 角色分别校验。核心仅允许 `csr`；主机过程宏额外允许其 `__internal_erase_components` 内部标志。拒绝 full、SSR、hydrate/hydration、server_fn 等重新进入实际框架图，同时不误禁其他依赖的普通 default feature。`graph --product-root .` 可单独检查并输出 JSON；构建清单记录通过检查的角色/路径/feature。
- Oxc 解析 JavaScript、检查语法/作用域错误、按真实 AST 识别 import/export、动态 import 与 `new URL("literal", import.meta.url)`；然后压缩与输出。字符串、注释、正则中的伪 import 不会当成依赖。
- 接受自包含 `.js`/`.mjs` ESM、静态 CSS import、`?url` 默认导入/默认再导出、字面量动态 import。别名必须精确配置；拒绝未 vendor 的裸导入、CommonJS `require`、计算型动态导入、import attributes、TS/JSX 和 ESM 环。这个工具不替代通用 npm 转译器；有这些输入时使用上游提供并审查过的独立 ESM 发行文件。
- Lightning CSS 解析、展开普通 `@import`、按真实 URL 值重写字体/图片，再压缩输出。不静默丢弃条件 `@import`；需先明确整理输入。CSS 导入顺序保持入口模块遍历顺序。
- `web/public` 按原目录复制到输出根目录。CSS 的绝对 URL 优先匹配 public 文件并保留固定 URL（例如 `/legacy/bannerWave1.png`）；ESM 绝对导入优先匹配 Web 源文件，其次在 public 内查找。引用 public 资源不会额外生成一份哈希文件；固定资源仍计入预算、源码漂移检查和 receipt。根 URL 拒绝 `..`、协议相对 URL、符号链接及越界路径，public 不能覆盖生成入口或追溯清单。
- 生成独立 WASM、确定性 gzip（mtime 0）、对应 WASM preload、字体 preload、独立 stylesheet，并保留原 inline font boot。不开启 source map，不引入远端 script/style。
- 默认单文件预算：普通资源 512 KiB、WASM/其 gzip 4 MiB、字体 16 MiB；bundle 最终输出 16 MiB。配置可以收紧这些上限，不能放宽。没有 public 清单时，public 全部按这些原严格上限计入 bundle。复制资源、清单、许可、HTML 同样计入预算。
- `web/pkg/xcss-web-build.json` 与 `dist/assets/xcss-web-build.json` 保留 `format/rust/wasm_bindgen/package/profile/files/browser_inputs/shared_inputs/builder_sha256/cargo_packages` 字段，同时增加资源、其他 path/patch 依赖、工具源码清单。工具的运行时源码还会对比编译到执行文件中的源码字节，拒绝使用过期工具伪造构建来源。
- 所有 `web/vendor/<package>/SOURCE.json` 精确校验目录文件名和 SHA-256；增加未登记文件、改动实现或许可证都会失败。构建前后再次比较整套源代码/lock/vendor/资源清单，即使构建过程中同时更新 SOURCE.json 也不能绕过漂移检测。
- 验证通过后再替换生成的 pkg/dist。普通失败会恢复上一对输出；不宣称跨两个目录的重命名具备断电原子性。构建不可与另一个指向相同输出的构建并发执行。
- `assets/vendor-sources.json` 保存完整的 browser vendor 来源。设置 `XSSI_BUILD_TOOLS_DIR` 与 `XSSI_FRONTEND_METADATA_FILE` 时生成 runtime receipt，字段为 `format_version=1`、`distribution`、`modules`、`outputs`、`rust`，模块 SHA 和输出 SHA 均绑定本次实际字节。`rust` 记录 WASM 输出、全部本地 Rust 输入及 Cargo.lock 摘要、工具链、目标、profile、feature 选项，以及 `cargo tree --edges normal,no-proc-macro` 的所选运行图；同一构建定义进入嵌入 Server 的资源 provenance。发布器在私有快照中重新查询并核对图，生成独立 WASM Cargo SBOM 和原始许可，绑定最终 Server 字节；host macro、build、dev 依赖不作为浏览器运行组件。两个环境变量需同时提供、绝对路径，receipt 必须创建为新文件。
- 输入/已有输出拒绝符号链接、硬链接、特殊文件，单文件读取上限 32 MiB，目录枚举上限 16,384 文件。不跟随 `target/.git/__pycache__/.pytest_cache`。

存在已审查的大型 public 媒体时，可在 `[web]` 明确设置 `public_assets_manifest = "public-assets.json"`，并在 `web/public-assets.json` 登记 **全部** public 文件，包括 README、文本和代码：

```json
{"format":1,"files":{"legacy/image.jpg":"64位原文件SHA256","legacy/README.md":"64位原文件SHA256"}}
```

清单必须直接位于 `web/`，使用 public 相对路径与真实 SHA-256。缺少文件、多出文件、字节变化、重复路径、未知字段或越界路径都会失败。核验完整清单后，按原始文件路径由 `mime_guess` 判定为 `image/*`、`video/*`、`audio/*` 的 public 输出进入独立媒体桶：单文件硬上限 4 MiB，合计硬上限 8 MiB，不能配置放大。其它 public 文件仍进入 bundle，JS/CSS/文本仍受普通资源 512 KiB 门，字体仍受字体与 bundle 门；所有输出按唯一输出路径计数，bundle 16 MiB 加 media 8 MiB 的总量最多 24 MiB。

清单与全部源文件均绑定构建前后源码快照，所有输出继续写入完整 receipt。CSS/ESM 对 public 媒体的引用只指向其固定输出，不另外复制哈希版本。该合同补上旧 Vite 只检查 bundle、未把直接复制的 public 纳入该预算的缺口；不表示旧版全部输出曾通过 16 MiB 上限。

## 开发与预览

`serve --debug` 首次构建后启动 Axum 服务。文件修改用名称/大小/mtime/ctime/inode 探测并 debounce；只有实际构建时才重新哈希所有源码与大字体。Rust/CSS/入口/vendor 更新触发完整资源重建；编译失败继续提供上次成功的资源，修改后自动恢复。浏览器手动刷新，不注入 Node/Vite HMR 运行时。

`preview` 只提供已生成输出，不自动构建或监听。`serve --skip-build` 跳过第一次构建但保留后续监听。监听成功会在 stdout 写一行 JSON，例如 `{"event":"listening","address":"http://127.0.0.1:5173","status":"/__rust_web/status"}`，便于浏览器测试读取动态端口。状态端点返回 `building/generation/last_error`，无缓存。

默认只绑定 loopback，拒绝公开监听携带凭据的开发代理。代理使用 reqwest 流式转发请求/响应，保留 Host、Origin、Cookie、Set-Cookie、状态码和跳转位置，移除逐跳头与 Connection 声明的逐跳字段；不自动跟随重定向。目标只接受 loopback HTTP。`/api=http://127.0.0.1:8080` 保留 `/api/...`；目标以 `/` 结尾则用目标路径替换匹配前缀，适合 `/media-hls=http://127.0.0.1:8888/`。WebSocket 代理当前明确拒绝，已有独立后端 WebSocket URL 应直接连接；HTTP HLS/WHEP 请求仍支持。

静态服务提供精确 MIME、nosniff、HTML no-store、ETag/HEAD/304 与 WASM gzip 协商（显式 `gzip;q=0` 优先于通配符）。生产 CSP、认证与部署继续由原 Axum 后端负责。

## Vendor 更新

```sh
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- vendor \
  --product-root . --source /reviewed/package/dist --destination vendor/example \
  --origin https://github.com/owner/repository --revision IMMUTABLE_ID \
  --include module.mjs --include LICENSE
cargo run --locked --manifest-path tools/web-build/Cargo.toml -- check-vendor --product-root .
```

`vendor` 只复制明确列出的相对文件/目录，必须包含许可，不覆盖现有目标；更新需先审查具体差异。生产项目还需保留 npm 原发行包 URL/integrity、包名版本、archive-prefix 与原文件映射，以便发布 SBOM 验证。此命令不代替上游发行档案真实性验证。

工具自身测试：`cargo test --locked --offline --manifest-path tools/web-build/Cargo.toml`。测试覆盖 AST 假 import/压缩、模块资源路径、CSS/字体/WASM 预载、预算、vendor 改动、链接拒绝、失败发布回滚、HTTP 条件请求与路径逃逸。
