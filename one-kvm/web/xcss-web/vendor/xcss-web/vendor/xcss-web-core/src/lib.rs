#![deny(missing_docs)]

//! Browser CSR runtime used by the public `xcss-web` crate.
//! Use `xcss_web::prelude::*` and the DOM builders from that facade.
//! See the package README for source and license history.

#![cfg_attr(all(feature = "nightly", rustc_nightly), feature(fn_traits))]
#![cfg_attr(all(feature = "nightly", rustc_nightly), feature(unboxed_closures))]

extern crate self as xcss_web;

/// Exports all the core types of the library.
pub mod prelude {
    // Traits
    // These should always be exported from the prelude
    pub use reactive_graph::prelude::*;
    pub use tachys::prelude::*;

    // Structs
    // In the future, maybe we should remove this blanket export
    // However, it is definitely useful relative to looking up every struct etc.
    mod export_types {
        #[cfg(feature = "nonce")]
        pub use crate::nonce::*;
        pub use crate::{
            callback::*, children::*, component::*, control_flow::*, error::*,
            into_view::*, mount::*, text_prop::*,
        };
        #[cfg(feature = "full")]
        pub use crate::{form::*, hydration::*, suspense::*};
        #[cfg(feature = "full")]
        pub use leptos_config::*;
        pub use leptos_dom::helpers::*;
        pub use xcss_web_macro::*;
        #[cfg(feature = "full")]
        pub use leptos_server::*;
        pub use oco_ref::*;
        pub use reactive_graph::{
            actions::*,
            computed::*,
            effect::*,
            graph::untrack,
            owner::*,
            signal::*,
            wrappers::{read::*, write::*},
        };
        #[cfg(feature = "full")]
        pub use server_fn::{
            self,
            error::{FromServerFnError, ServerFnError, ServerFnErrorErr},
        };
        pub use tachys::{
            reactive_graph::{bind::BindAttribute, node_ref::*, Suspend},
            view::{fragment::Fragment, template::ViewTemplate},
        };
    }
    pub use export_types::*;
}

/// Components used for working with HTML forms, like `<ActionForm>`.
#[cfg(feature = "full")]
pub mod form;

/// A standard way to wrap functions and closures to pass them to components.
pub use reactive_graph::callback;

/// Types that can be passed as the `children` prop of a component.
pub mod children;

/// Wrapper for intercepting component attributes.
pub mod attribute_interceptor;

#[doc(hidden)]
/// Traits used to implement component constructors.
pub mod component;
#[cfg(feature = "full")]
mod error_boundary;

/// Tools for handling errors.
pub mod error {
    #[cfg(feature = "full")]
    pub use crate::error_boundary::*;
    pub use throw_error::*;
}

/// Control-flow components like `<Show>`, `<For>`, and `<Await>`.
pub mod control_flow {
    #[cfg(feature = "full")]
    pub use crate::await_::*;
    pub use crate::{animated_show::*, for_loop::*, show::*, show_let::*};
}
mod animated_show;
#[cfg(feature = "full")]
mod await_;
mod for_loop;
mod show;
mod show_let;

/// A component that allows rendering a component somewhere else.
pub mod portal;

/// Components to enable server-side rendering and client-side hydration.
#[cfg(feature = "full")]
pub mod hydration;

/// Utilities for exporting nonces to be used for a Content Security Policy.
#[cfg(feature = "nonce")]
pub mod nonce;

/// Components to load asynchronous data.
#[cfg(feature = "full")]
pub mod suspense {
    pub use crate::{suspense_component::*, transition::*};
}

#[cfg(feature = "full")]
#[macro_use]
mod suspense_component;

/// Types for reactive string properties for components.
pub mod text_prop;
#[cfg(feature = "full")]
mod transition;
pub use xcss_web_macro::*;
#[doc(inline)]
#[cfg(feature = "full")]
pub use server_fn;
#[doc(hidden)]
pub use typed_builder;
#[doc(hidden)]
pub use typed_builder_macro;
mod into_view;
pub use into_view::IntoView;
#[doc(inline)]
pub use leptos_dom;
mod provider;
#[doc(inline)]
pub use tachys;
/// Tools to mount an application to the DOM, or to hydrate it from server-rendered HTML.
pub mod mount;
#[doc(inline)]
#[cfg(feature = "full")]
pub use leptos_config as config;
#[doc(inline)]
pub use oco_ref as oco;
mod from_form_data;
#[doc(inline)]
pub use either_of as either;
#[doc(inline)]
pub use reactive_graph as reactive;

/// Provide and access data along the reactive graph, sharing data without directly passing arguments.
pub mod context {
    pub use crate::provider::*;
    pub use reactive_graph::owner::{provide_context, use_context};
}

#[doc(inline)]
#[cfg(feature = "full")]
pub use leptos_server as server;
/// HTML attribute types.
#[doc(inline)]
pub use tachys::html::attribute as attr;
/// HTML element types.
#[doc(inline)]
pub use tachys::html::element as html;
/// HTML event types.
#[doc(no_inline)]
pub use tachys::html::event as ev;
/// MathML element types.
#[doc(inline)]
pub use tachys::mathml as math;
/// SVG element types.
#[doc(inline)]
pub use tachys::svg;

#[cfg(feature = "subsecond")]
/// Utilities for using binary hot-patching with [`subsecond`].
pub mod subsecond;

/// Utilities for simple isomorphic logging to the console or terminal.
pub mod logging {
    pub use leptos_dom::{
        debug_error, debug_log, debug_warn, error, log, warn,
    };
}

/// Utilities for working with asynchronous tasks.
pub mod task {
    use any_spawner::Executor;
    use reactive_graph::computed::ScopedFuture;
    use std::future::Future;

    /// Spawns a thread-safe [`Future`].
    ///
    /// This will be run with the current reactive owner and observer using a [`ScopedFuture`].
    #[track_caller]
    #[inline(always)]
    pub fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        let fut = ScopedFuture::new(fut);

        #[cfg(not(target_family = "wasm"))]
        Executor::spawn(fut);

        #[cfg(target_family = "wasm")]
        Executor::spawn_local(fut);
    }

    /// Spawns a [`Future`] that cannot be sent across threads.
    #[track_caller]
    #[inline(always)]
    pub fn spawn_local(fut: impl Future<Output = ()> + 'static) {
        Executor::spawn_local(fut)
    }

    /// Waits until the next "tick" of the current async executor.
    pub async fn tick() {
        Executor::tick().await
    }

    pub use reactive_graph::{
        spawn_local_scoped, spawn_local_scoped_with_cancellation,
    };
}

// these reexports are used in islands
#[cfg(feature = "islands")]
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use serde_json;
#[cfg(feature = "tracing")]
#[doc(hidden)]
pub use tracing;
#[doc(hidden)]
pub use wasm_bindgen;
#[doc(hidden)]
pub use wasm_split_helpers as wasm_split;
#[doc(hidden)]
pub use web_sys;

#[doc(hidden)]
pub mod __reexports {
    pub use send_wrapper;
    pub use wasm_bindgen_futures;
}

#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct PrefetchLazyFn(
    pub  reactive_graph::owner::ArcStoredValue<
        std::collections::HashSet<&'static str>,
    >,
);

#[doc(hidden)]
pub fn prefetch_lazy_fn_on_server(id: &'static str) {
    use crate::context::use_context;
    use reactive_graph::traits::WriteValue;

    if let Some(prefetches) = use_context::<PrefetchLazyFn>() {
        prefetches.0.write_value().insert(id);
    }
}

#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct WasmSplitManifest(
    pub  reactive_graph::owner::ArcStoredValue<(
        String,                                         // the pkg root
        std::collections::HashMap<String, Vec<String>>, // preloads
        String, // the name of the __wasm_split.js file
    )>,
);
