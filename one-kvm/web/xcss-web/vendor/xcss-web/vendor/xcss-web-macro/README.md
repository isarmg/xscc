# xcss-web-macro

`xcss-web` 的内部浏览器 CSR 过程宏。产品只依赖公开入口 `xcss-web`。

## 来源与许可证

此代码基于 Leptos 0.8.19 的裁剪版本；沿用原 MIT 许可证，完整通知见同目录 `LICENSE`。以下保留原来源、修改记录和原文件摘要。`SOURCE.json` 验证当前源码与本 README 的 SHA-256。

```json xcss-upstream
{
  "added_license": {
    "path": "LICENSE",
    "reason": "The published macro archive declares MIT but does not include a LICENSE file; preserve the official project notice",
    "revision": "fbc9ed631ff8c19c5eed02a1d4459e202cff7043",
    "sha256": "76356e713a0d1ea0bde549f4236b01c750100ab31790019843faa5aec449310f",
    "source": "https://github.com/leptos-rs/leptos"
  },
  "changes": [
    "Remove the unused uuid/v4 dependency; no uuid references occur in the published src or build.rs",
    "Make server_fn_macro optional and expose the server attribute macro only with full",
    "Keep full enabled by default; serverfn and existing server_fn_macro forwarding features explicitly restore full",
    "Leave csr empty and preserve all view/component/hot-reload implementations",
    "Add an empty workspace table so the normalized release crate remains standalone when stored inside the upstream monorepo"
  ],
  "format": 1,
  "omitted_registry_files": {
    ".cargo-ok": "Cargo registry extraction marker, not a published source input",
    ".cargo_vcs_info.json": "Recorded as upstream_vcs_info in SOURCE.json",
    "Cargo.lock": "Published standalone crate lock is not used by path dependency consumers; product locks remain authoritative"
  },
  "original_files": {
    ".gitignore": "7638f044db68ce808e7568297a91c0e4815d30c4a48237461259942b2778a210",
    "Cargo.toml": "04630a2606a5db6b2b98dcdfbcbd6aa3dac659308eb5f11f2964fcff5dd59f07",
    "Cargo.toml.orig": "94db5088087d82cd26b9742dfe3eb078db36b4a01dd786fee5f33bfff7591a4c",
    "Makefile.toml": "f698a039db6082bf80d4148215ec6e0122cfa151cec50078702afb4598455c0b",
    "README.md": "dfc971183cb86e19ac99f0e327fdd23f18614b4941e536be68de7af334d0179c",
    "build.rs": "11ccc42a6266f3bb42677ff796dd8f456a72460f0a20a83cce22bb29dfec534e",
    "src/component.rs": "44408dc3332f6d7920f7c37d4026665e237f5604c6c6bd5bf7348782bfc6cd81",
    "src/diagnostics.rs": "f475c804c45337cff5c3a23bdc5be2bc2c9b2d36b1f189f129cb8fb51f336157",
    "src/lazy.rs": "cbbf907a6956e517dbfca671cd0374ce6913b909d559e154802fba357dafe708",
    "src/lib.rs": "f289c8b7fd2fc152e63928b2bcd86bea1548ffc6359011cc363a411e44fb22d8",
    "src/memo.rs": "bc14eb8b81c3708d83ef5b82b13eb60dc8350e62b9c75702e649b706a1025c02",
    "src/params.rs": "ea146d340b07629cf309dbb30a39b12286638d3b145f56e2cd91938011b92d6a",
    "src/slice.rs": "c34ae1904fdb46ac10e6dd7c0bab6f1aae12cf86b43eb645fcfb312cc0f9c969",
    "src/slot.rs": "a23677437ff50cd6b1adea8632de1a36089d175df82064c4498e366d44bcc23b",
    "src/view/component_builder.rs": "e9fea49ca71668f066d5377c8969cb0b724cf7aee8418bce2da08f30fadf3d1c",
    "src/view/mod.rs": "13c716bd0114b31192192cf102fc010622edc5069338be4f9633d92570d6aa65",
    "src/view/slot_helper.rs": "6fdfcaec96dd5bc5a98518c3b18a29992e1df6c3cb38d29ab762788af92d7cd7",
    "src/view/snapshots/leptos_macro__view__tests__client_template__full_span__counter_component.snap": "c4326632e924aefccf0da186b4f7e5416978113d7839ed563989503b8d0c50ac",
    "src/view/utils.rs": "7a727b6fd237b85223d972df453f26a38c89777160d725addb8e0a7b546d40db",
    "tests/component.rs": "1cbac097e82d148b95b968586eb2bbd8ea37eb95e52c1902c4e3b009bb9c741a",
    "tests/memo.rs": "fb3294c3723bbd52218bc06208a1ca19a6e546c2d2d89104806eec5bbc3944cf",
    "tests/memo/red.rs": "55cf382d770759de2a22323dfc0ce81b9917b2d54df75bc057ebb06ace130897",
    "tests/memo/red.stderr": "ddc012f0774761f7c5e6e211a3b76956398bf47c7cad5f035837ca5ad826c59e",
    "tests/params.rs": "faa4fae099e9fbe973abac257ab058f9d3b00e05dc5edaf7d15c145491d2fd58",
    "tests/server.rs": "7dae1c1fad4c74a74c8c99407245275bd5f572c8df90cec82f5e9e7e31444823",
    "tests/slice.rs": "1b11589dc5cec87d2573007f8fcb36e84d7caa69770eda2e7b4f5c1002ccb241",
    "tests/slice/red.rs": "8ef5511894e823afa9362038f584ebb52f93b2461327535710f6f6b810c1c9f0",
    "tests/slice/red.stderr": "60b5da45fcb955b1b945f9eb23a2f62ac7d53ad612e2b4cad929ee6c40ec0d67",
    "tests/ui.rs": "37e9ea409d7f24f6d61b2710c318bbf39b1ade924c1aebf0479643ee0e5b92e8",
    "tests/ui/component.rs": "96d45374838c0beda650ed34d555ddfc7c48adf596504edde2f0cfd57ac21450",
    "tests/ui/component.stderr": "fbca69941b51afd7d2ad6636b16014c8dc81fca2ee11d83f20807099d061032b",
    "tests/ui/component_absolute.rs": "e2dfd2810cb39fb701d17dbe2b5451b07ee670cb93b761cdcd673fd7d9faadcd",
    "tests/ui/component_absolute.stderr": "ae8db8e420ee62270d7c502b81fd854b884de57a523539f91e2bf41d160c7b65",
    "tests/ui/diagnostic_help.rs": "a46a604d8d74d55b18cb70b5c1ad4ae5e2e7e63c33c211c06e208d4c6ebaea89",
    "tests/ui/diagnostic_help.stderr": "dfcabd45a39339eadd5c4cb5be6ce8feba52d12199d67ae50f86e2c99e1f5970",
    "tests/ui/server.rs": "f0d35fbb4be052ed38f601161b89c7b51ddb9b23cdf5cf0ab8f783ebe5f1198e",
    "tests/ui/server.stderr": "2ea307bd7ebfc988ad49ec029d327632ac875c9bc1c70466f37e07725c009695",
    "tests/ui/view_errors.rs": "c4178ee74ba1263043911a9d8d3ceeda1f1c29b568a2d16330d71a9b751252ba",
    "tests/ui/view_errors.stderr": "6ad91cb401579345610cb021878d01a27933e4bcc2445d188a25948bf596060a"
  },
  "package": "leptos_macro",
  "patch_file": "leptos-macro-0.8.19-csr-minimal.patch",
  "patch_sha256": "f8f45e4e6bc01019c5ada0c7d5d94767413ee18587582cb11026ea49f0bd31d5",
  "previous_files": {
    ".gitignore": "7638f044db68ce808e7568297a91c0e4815d30c4a48237461259942b2778a210",
    "Cargo.toml": "369af0e75f7ca0550b8bdf399c296dec2ba4d2138beb3068a8b0d373a23a29c4",
    "Cargo.toml.orig": "94db5088087d82cd26b9742dfe3eb078db36b4a01dd786fee5f33bfff7591a4c",
    "LICENSE": "76356e713a0d1ea0bde549f4236b01c750100ab31790019843faa5aec449310f",
    "Makefile.toml": "f698a039db6082bf80d4148215ec6e0122cfa151cec50078702afb4598455c0b",
    "README.md": "dfc971183cb86e19ac99f0e327fdd23f18614b4941e536be68de7af334d0179c",
    "XCSS_CSR_CLEANUP.patch": "4211f0b68ae5511589668076f83db59b469fddfed78cb8ca992e8abf3b8ff7f8",
    "build.rs": "11ccc42a6266f3bb42677ff796dd8f456a72460f0a20a83cce22bb29dfec534e",
    "src/component.rs": "44408dc3332f6d7920f7c37d4026665e237f5604c6c6bd5bf7348782bfc6cd81",
    "src/diagnostics.rs": "f475c804c45337cff5c3a23bdc5be2bc2c9b2d36b1f189f129cb8fb51f336157",
    "src/lazy.rs": "cbbf907a6956e517dbfca671cd0374ce6913b909d559e154802fba357dafe708",
    "src/lib.rs": "4b4da8c14fd55793cb0e8c2fb548db8a5eb3de8193fb1f78201e16be68beb121",
    "src/memo.rs": "bc14eb8b81c3708d83ef5b82b13eb60dc8350e62b9c75702e649b706a1025c02",
    "src/params.rs": "ea146d340b07629cf309dbb30a39b12286638d3b145f56e2cd91938011b92d6a",
    "src/slice.rs": "c34ae1904fdb46ac10e6dd7c0bab6f1aae12cf86b43eb645fcfb312cc0f9c969",
    "src/slot.rs": "a23677437ff50cd6b1adea8632de1a36089d175df82064c4498e366d44bcc23b",
    "src/view/component_builder.rs": "e9fea49ca71668f066d5377c8969cb0b724cf7aee8418bce2da08f30fadf3d1c",
    "src/view/mod.rs": "0349a120799bc0cb6668697730640bddfdc7c70fcaf529dc7ff151a935953758",
    "src/view/slot_helper.rs": "6fdfcaec96dd5bc5a98518c3b18a29992e1df6c3cb38d29ab762788af92d7cd7",
    "src/view/snapshots/leptos_macro__view__tests__client_template__full_span__counter_component.snap": "c4326632e924aefccf0da186b4f7e5416978113d7839ed563989503b8d0c50ac",
    "src/view/utils.rs": "7a727b6fd237b85223d972df453f26a38c89777160d725addb8e0a7b546d40db",
    "tests/component.rs": "1cbac097e82d148b95b968586eb2bbd8ea37eb95e52c1902c4e3b009bb9c741a",
    "tests/memo.rs": "fb3294c3723bbd52218bc06208a1ca19a6e546c2d2d89104806eec5bbc3944cf",
    "tests/memo/red.rs": "55cf382d770759de2a22323dfc0ce81b9917b2d54df75bc057ebb06ace130897",
    "tests/memo/red.stderr": "ddc012f0774761f7c5e6e211a3b76956398bf47c7cad5f035837ca5ad826c59e",
    "tests/params.rs": "faa4fae099e9fbe973abac257ab058f9d3b00e05dc5edaf7d15c145491d2fd58",
    "tests/server.rs": "7dae1c1fad4c74a74c8c99407245275bd5f572c8df90cec82f5e9e7e31444823",
    "tests/slice.rs": "1b11589dc5cec87d2573007f8fcb36e84d7caa69770eda2e7b4f5c1002ccb241",
    "tests/slice/red.rs": "8ef5511894e823afa9362038f584ebb52f93b2461327535710f6f6b810c1c9f0",
    "tests/slice/red.stderr": "60b5da45fcb955b1b945f9eb23a2f62ac7d53ad612e2b4cad929ee6c40ec0d67",
    "tests/ui.rs": "37e9ea409d7f24f6d61b2710c318bbf39b1ade924c1aebf0479643ee0e5b92e8",
    "tests/ui/component.rs": "96d45374838c0beda650ed34d555ddfc7c48adf596504edde2f0cfd57ac21450",
    "tests/ui/component.stderr": "fbca69941b51afd7d2ad6636b16014c8dc81fca2ee11d83f20807099d061032b",
    "tests/ui/component_absolute.rs": "e2dfd2810cb39fb701d17dbe2b5451b07ee670cb93b761cdcd673fd7d9faadcd",
    "tests/ui/component_absolute.stderr": "ae8db8e420ee62270d7c502b81fd854b884de57a523539f91e2bf41d160c7b65",
    "tests/ui/diagnostic_help.rs": "a46a604d8d74d55b18cb70b5c1ad4ae5e2e7e63c33c211c06e208d4c6ebaea89",
    "tests/ui/diagnostic_help.stderr": "dfcabd45a39339eadd5c4cb5be6ce8feba52d12199d67ae50f86e2c99e1f5970",
    "tests/ui/server.rs": "f0d35fbb4be052ed38f601161b89c7b51ddb9b23cdf5cf0ab8f783ebe5f1198e",
    "tests/ui/server.stderr": "2ea307bd7ebfc988ad49ec029d327632ac875c9bc1c70466f37e07725c009695",
    "tests/ui/view_errors.rs": "c4178ee74ba1263043911a9d8d3ceeda1f1c29b568a2d16330d71a9b751252ba",
    "tests/ui/view_errors.stderr": "6ad91cb401579345610cb021878d01a27933e4bcc2445d188a25948bf596060a"
  },
  "registry_archive_sha256": "0902af1e6947ffc7e906b7003a165f76f178dc711a47b74241025090b0c32a72",
  "registry_source": "registry+https://github.com/rust-lang/crates.io-index",
  "upstream": "https://github.com/leptos-rs/leptos",
  "upstream_revision": "58fe6c1ad0cd175e9c8ec204895a9739d52fda16",
  "upstream_vcs_info": {
    "git": {
      "sha1": "58fe6c1ad0cd175e9c8ec204895a9739d52fda16"
    },
    "path_in_vcs": "leptos_macro"
  },
  "version": "0.8.19",
  "xcss_integration": {
    "base_source_manifest_sha256": "25f9e5a2573450ba082155ff1ebef859a6ffa8c60583b7f9c1c6160c97dfe945",
    "changes": [
      "Gate server-only ToTokens import behind full",
      "Gate ident_from_tag_name with debug_assertions exactly like its caller; preserve debug code generation"
    ],
    "patch_file": "XCSS_CSR_CLEANUP.patch",
    "patch_sha256": "4211f0b68ae5511589668076f83db59b469fddfed78cb8ca992e8abf3b8ff7f8"
  },
  "xcss_rename": {
    "changes": [
      "Rename package, Rust paths and framework-facing runtime markers",
      "Expose the runtime and component macros through the xcss-web facade",
      "Keep MIT notices and previous source/patch metadata in this README"
    ],
    "internal_package": "xcss-web-macro",
    "public_package": "xcss-web",
    "runtime": "csr"
  }
}
```

## 历史记录：README.md

```text
<picture>
    <source srcset="https://raw.githubusercontent.com/leptos-rs/leptos/main/docs/logos/Leptos_logo_pref_dark_RGB.svg" media="(prefers-color-scheme: dark)">
    <img src="https://raw.githubusercontent.com/leptos-rs/leptos/main/docs/logos/Leptos_logo_RGB.svg" alt="Leptos Logo">
</picture>

[![crates.io](https://img.shields.io/crates/v/leptos.svg)](https://crates.io/crates/leptos)
[![docs.rs](https://docs.rs/leptos/badge.svg)](https://docs.rs/leptos)
![Crates.io MSRV](https://img.shields.io/crates/msrv/leptos)
[![Discord](https://img.shields.io/discord/1031524867910148188?color=%237289DA&label=discord)](https://discord.gg/YdRAhS7eQB)
[![Matrix](https://img.shields.io/badge/Matrix-leptos-grey?logo=matrix&labelColor=white&logoColor=black)](https://matrix.to/#/#leptos:matrix.org)

[Website](https://leptos.dev) | [Book](https://leptos-rs.github.io/leptos/) | [Docs.rs](https://docs.rs/leptos/latest/leptos/) | [Playground](https://codesandbox.io/p/devbox/playground-j23dz7?file=%2Fsrc%2Fmain.rs) | [Discord](https://discord.gg/YdRAhS7eQB)

You can find a list of useful libraries and example projects at [`awesome-leptos`](https://github.com/leptos-rs/awesome-leptos).

# Leptos

```rust
use leptos::*;

#[component]
pub fn SimpleCounter(initial_value: i32) -> impl IntoView {
    // create a reactive signal with the initial value
    let (value, set_value) = signal(initial_value);

    // create event handlers for our buttons
    // note that `value` and `set_value` are `Copy`, so it's super easy to move them into closures
    let clear = move |_| set_value(0);
    let decrement = move |_| set_value.update(|value| *value -= 1);
    let increment = move |_| set_value.update(|value| *value += 1);

    // create user interfaces with the declarative `view!` macro
    view! {
        <div>
            <button on:click=clear>Clear</button>
            <button on:click=decrement>-1</button>
            // text nodes can be quoted or unquoted
            <span>"Value: " {value} "!"</span>
            <button on:click=increment>+1</button>
        </div>
    }
}

// we also support a builder syntax rather than the JSX-like `view` macro
#[component]
pub fn SimpleCounterWithBuilder(initial_value: i32) -> impl IntoView {
    use leptos::html::*;

    let (value, set_value) = signal(initial_value);
    let clear = move |_| set_value(0);
    let decrement = move |_| set_value.update(|value| *value -= 1);
    let increment = move |_| set_value.update(|value| *value += 1);

    // the `view` macro above expands to this builder syntax
    div().child((
        button().on(ev::click, clear).child("Clear"),
        button().on(ev::click, decrement).child("-1"),
        span().child(("Value: ", value, "!")),
        button().on(ev::click, increment).child("+1")
    ))
}

// Easy to use with Trunk (trunk-rs.github.io/trunk) or with a simple wasm-bindgen setup
pub fn main() {
    mount_to_body(|| view! {
        <SimpleCounter initial_value=3 />
    })
}
```

## About the Framework

Leptos is a full-stack, isomorphic Rust web framework leveraging fine-grained reactivity to build declarative user interfaces.

## What does that mean?

- **Full-stack**: Leptos can be used to build apps that run in the browser (client-side rendering), on the server (server-side rendering), or by rendering HTML on the server and then adding interactivity in the browser (server-side rendering with hydration). This includes support for HTTP streaming of both data ([`Resource`s](https://docs.rs/leptos/latest/leptos/prelude/struct.Resource.html)) and HTML (out-of-order or in-order streaming of [`<Suspense/>`](https://docs.rs/leptos/latest/leptos/suspense/fn.Suspense.html) components.)
- **Isomorphic**: Leptos provides primitives to write isomorphic [server functions](https://docs.rs/server_fn/latest/server_fn/), i.e., functions that can be called with the “same shape” on the client or server, but only run on the server. This means you can write your server-only logic (database requests, authentication etc.) alongside the client-side components that will consume it, and call server functions as if they were running in the browser, without needing to create and maintain a separate REST or other API.
- **Web**: Leptos is built on the Web platform and Web standards. The [router](https://docs.rs/leptos_router/latest/leptos_router/) is designed to use Web fundamentals (like links and forms) and build on top of them rather than trying to replace them.
- **Framework**: Leptos provides most of what you need to build a modern web app: a reactive system, templating library, and a router that works on both the server and client side.
- **Fine-grained reactivity**: The entire framework is built from reactive primitives. This allows for extremely performant code with minimal overhead: when a reactive signal’s value changes, it can update a single text node, toggle a single class, or remove an element from the DOM without any other code running. (So, no virtual DOM overhead!)
- **Declarative**: Tell Leptos how you want the page to look, and let the framework tell the browser how to do it.

## Learn more

Here are some resources for learning more about Leptos:

- [Book](https://leptos-rs.github.io/leptos/) (work in progress)
- [Examples](https://github.com/leptos-rs/leptos/tree/main/examples)
- [API Documentation](https://docs.rs/leptos/latest/leptos/)
- [Common Bugs](https://github.com/leptos-rs/leptos/tree/main/docs/COMMON_BUGS.md) (and how to fix them!)

### Random numbers on wasm (`rand` / `getrandom`)

When you compile a Leptos app to `wasm32-unknown-unknown`, `rand` and `getrandom` need a JavaScript-backed source of randomness. If that backend isn’t enabled, your build can fail or randomness just won’t work in the browser.

Leptos itself takes care of this for its own code, but that does **not** automatically configure your app’s own `rand` / `getrandom` dependencies. If you use them directly, you need to turn on the JS backend yourself.

A simple setup in your `Cargo.toml` might look like this:

```toml
[dependencies]
# Make sure getrandom works on wasm by enabling its JS backend
getrandom = { version = "0.2", features = ["js"] }
rand      = { version = "0.8", features = ["small_rng"] }
```

Some of the examples in this repo (for example `js-framework-benchmark` and `hackernews_js_fetch`) already do this, so you can use them as a reference if you’re unsure.

## `cargo-leptos`

[`cargo-leptos`](https://github.com/leptos-rs/cargo-leptos) is a build tool that's designed to make it easy to build apps that run on both the client and the server, with seamless integration. The best way to get started with a real Leptos project right now is to use `cargo-leptos` and our starter templates for [Actix](https://github.com/leptos-rs/start) or [Axum](https://github.com/leptos-rs/start-axum).

```bash
cargo install cargo-leptos --locked
cargo leptos new --git https://github.com/leptos-rs/start-axum
cd [your project name]
cargo leptos watch
```

Open browser to [http://localhost:3000/](http://localhost:3000/).

## FAQs

### What’s up with the name?

_Leptos_ (λεπτός) is an ancient Greek word meaning “thin, light, refined, fine-grained.” To me, a classicist and not a dog owner, it evokes the lightweight reactive system that powers the framework. I've since learned the same word is at the root of the medical term “leptospirosis,” a blood infection that affects humans and animals... My bad. No dogs were harmed in the creation of this framework.

### Is it production ready?

People usually mean one of three things by this question.

1. **Are the APIs stable?** i.e., will I have to rewrite my whole app from Leptos 0.1 to 0.2 to 0.3 to 0.4, or can I write it now and benefit from new features and updates as new versions come?

The APIs are basically settled. We’re adding new features, but we’re very happy with where the type system and patterns have landed. I would not expect major breaking changes to your code to adapt to future releases, in terms of architecture.

2. **Are there bugs?**

Yes, I’m sure there are. You can see from the state of our issue tracker over time that there aren’t that _many_ bugs and they’re usually resolved pretty quickly. But for sure, there may be moments where you encounter something that requires a fix at the framework level, which may not be immediately resolved.

3. **Am I a consumer or a contributor?**

This may be the big one: “production ready” implies a certain orientation to a library: that you can basically use it, without any special knowledge of its internals or ability to contribute. Everyone has this at some level in their stack: for example I (@gbj) don’t have the capacity or knowledge to contribute to something like `wasm-bindgen` at this point: I simply rely on it to work.

There are several people in the community using Leptos right now for many websites at work, who have also become significant contributors. There may be missing features that you need, and you may end up building them! But, if you're willing to contribute a few missing pieces along the way, the framework is most definitely usable for production applications, especially given the ecosystem of libraries that have sprung up around it.

### Can I use this for native GUI?

Sure! Obviously the `view` macro is for generating DOM nodes but you can use the reactive system to drive any native GUI toolkit that uses the same kind of object-oriented, event-callback-based framework as the DOM pretty easily. The principles are the same:

- Use signals, derived signals, and memos to create your reactive system
- Create GUI widgets
- Use event listeners to update signals
- Create effects to update the UI

The 0.7 update originally set out to create a "generic rendering" approach that would allow us to reuse most of the same view logic to do all of the above. Unfortunately, this has had to be shelved for now due to difficulties encountered by the Rust compiler when building larger-scale applications with the number of generics spread throughout the codebase that this required. It's an approach I'm looking forward to exploring again in the future; feel free to reach out if you're interested in this kind of work.

### How is this different from Yew?

Yew is the most-used library for Rust web UI development, but there are several differences between Yew and Leptos, in philosophy, approach, and performance.

- **VDOM vs. fine-grained:** Yew is built on the virtual DOM (VDOM) model: state changes cause components to re-render, generating a new virtual DOM tree. Yew diffs this against the previous VDOM, and applies those patches to the actual DOM. Component functions rerun whenever state changes. Leptos takes an entirely different approach. Components run once, creating (and returning) actual DOM nodes and setting up a reactive system to update those DOM nodes.
- **Performance:** This has huge performance implications: Leptos is simply much faster at both creating and updating the UI than Yew is.
- **Server integration:** Yew was created in an era in which browser-rendered single-page apps (SPAs) were the dominant paradigm. While Leptos supports client-side rendering, it also focuses on integrating with the server side of your application via server functions and multiple modes of serving HTML, including out-of-order streaming.

### How is this different from Dioxus?

Like Leptos, Dioxus is a framework for building UIs using web technologies. However, there are significant differences in approach and features.

- **VDOM vs. fine-grained:** While Dioxus has a performant virtual DOM (VDOM), it still uses coarse-grained/component-scoped reactivity: changing a stateful value reruns the component function and diffs the old UI against the new one. Leptos components use a different mental model, creating (and returning) actual DOM nodes and setting up a reactive system to update those DOM nodes.
- **Web vs. desktop priorities:** Dioxus uses Leptos server functions in its fullstack mode, but does not have the same `<Suspense>`-based support for things like streaming HTML rendering, or share the same focus on holistic web performance. Leptos tends to prioritize holistic web performance (streaming HTML rendering, smaller WASM binary sizes, etc.), whereas Dioxus has an unparalleled experience when building desktop apps, because your application logic runs as a native Rust binary.

### How is this different from Sycamore?

Sycamore and Leptos are both heavily influenced by SolidJS. At this point, Leptos has a larger community and ecosystem and is more actively developed. Other differences:

- **Templating DSLs:** Sycamore uses a custom templating language for its views, while Leptos uses a JSX-like template format.
- **`'static` signals:** One of Leptos’s main innovations was the creation of `Copy + 'static` signals, which have excellent ergonomics. Sycamore is in the process of adopting the same pattern, but this is not yet released.
- **Perseus vs. server functions:** The Perseus metaframework provides an opinionated way to build Sycamore apps that include server functionality. Leptos instead provides primitives like server functions in the core of the framework.

```

## 历史记录：Cargo.toml.orig

```text
[package]
name = "leptos_macro"
version = "0.8.19"
authors = ["Greg Johnston"]
license = "MIT"
repository = "https://github.com/leptos-rs/leptos"
description = "view macro for the Leptos web framework."
readme = "../README.md"
rust-version.workspace = true
edition.workspace = true

[lib]
proc-macro = true

[dependencies]
attribute-derive = { features = [
  "syn-full",
], workspace = true, default-features = true }
cfg-if = { workspace = true, default-features = true }
html-escape = { workspace = true, default-features = true }
itertools = { workspace = true, default-features = true }
prettyplease = { workspace = true, default-features = true }
proc-macro2 = { workspace = true, default-features = true }
quote = { workspace = true, default-features = true }
syn = { features = ["full"], workspace = true, default-features = true }
rstml = { workspace = true, default-features = true }
leptos_hot_reload = { workspace = true }
server_fn_macro = { workspace = true }
convert_case = { workspace = true, default-features = true }
convert_case_extras = { workspace = true, default-features = true }
uuid = { features = ["v4"], workspace = true, default-features = true }
tracing = { optional = true, workspace = true, default-features = true }

[dev-dependencies]
log = { workspace = true, default-features = true }
typed-builder = { workspace = true, default-features = true }
trybuild = { workspace = true, default-features = true }
leptos = { path = "../leptos" }
leptos_router = { path = "../router", features = ["ssr"] }
server_fn = { path = "../server_fn", features = ["cbor"] }
insta = { workspace = true, default-features = true }
serde = { workspace = true, default-features = true }

[build-dependencies]
rustc_version = { workspace = true, default-features = true }

[features]
csr = []
hydrate = []
ssr = ["server_fn_macro/ssr"]
nightly = ["server_fn_macro/nightly"]
tracing = ["dep:tracing"]
islands = []
trace-components = []
trace-component-props = []
actix = ["server_fn_macro/actix"]
axum = ["server_fn_macro/axum"]
generic = ["server_fn_macro/generic"]
# Having an erasure feature rather than normal --cfg erase_components for the proc macro crate is a workaround for this rust issue:
# https://github.com/rust-lang/cargo/issues/4423
# TLDR proc macros will ignore RUSTFLAGS when --target is specified on the cargo command.
# This works around the issue by the non proc-macro crate which does see RUSTFLAGS enabling the replacement feature on the proc-macro crate, which wouldn't.
# This is automatic as long as the leptos crate is depended upon,
# downstream usage should never manually enable this feature.
__internal_erase_components = []

[package.metadata.cargo-all-features]
denylist = ["tracing", "trace-component-props", "trace-components"]
skip_feature_sets = [
  [
    "csr",
    "hydrate",
  ],
  [
    "hydrate",
    "csr",
  ],
  [
    "hydrate",
    "ssr",
  ],
  [
    "actix",
    "axum",
  ],
  [
    "actix",
    "generic",
  ],
  [
    "generic",
    "axum",
  ],
  [
    "nightly",
  ],
]
max_combination_size = 2

[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--generate-link-to-definition"]

[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(rustc_nightly)'] }

```

## 历史记录：XCSS_CSR_CLEANUP.patch

以下 JSON 的 `patch` 字段保留原补丁完整文本，包括空白上下文与换行；将该字段按 UTF-8 编码即可还原原始补丁。

```json
{
  "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -11,7 +11,9 @@\n use component::DummyModel;\n use proc_macro::TokenStream;\n use proc_macro2::{Span, TokenTree};\n-use quote::{quote, ToTokens};\n+use quote::quote;\n+#[cfg(feature = \"full\")]\n+use quote::ToTokens;\n use std::str::FromStr;\n use syn::{spanned::Spanned, token::Pub, Visibility};\n \n--- a/src/view/mod.rs\n+++ b/src/view/mod.rs\n@@ -1947,6 +1947,7 @@\n     }\n }\n \n+#[cfg(debug_assertions)]\n pub(crate) fn ident_from_tag_name(\n     tag_name: &NodeName,\n     errors: &mut Errors,\n"
}
```
