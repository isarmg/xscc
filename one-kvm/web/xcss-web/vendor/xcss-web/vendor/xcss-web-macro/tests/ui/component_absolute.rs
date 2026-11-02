#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn missing_return_type() {}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn unknown_prop_option(#[prop(hello)] test: bool) -> impl ::xcss_web::IntoView {
    _ = test;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn optional_and_optional_no_strip(
    #[prop(optional, optional_no_strip)] conflicting: bool,
) -> impl IntoView {
    _ = conflicting;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn optional_and_strip_option(
    #[prop(optional, strip_option)] conflicting: bool,
) -> impl ::xcss_web::IntoView {
    _ = conflicting;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn optional_no_strip_and_strip_option(
    #[prop(optional_no_strip, strip_option)] conflicting: bool,
) -> impl ::xcss_web::IntoView {
    _ = conflicting;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn default_without_value(
    #[prop(default)] default: bool,
) -> impl ::xcss_web::IntoView {
    _ = default;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
fn default_with_invalid_value(
    #[prop(default= |)] default: bool,
) -> impl ::xcss_web::IntoView {
    _ = default;
}

#[cfg(all(feature = "nightly", rustc_nightly))]
#[::xcss_web::component]
pub fn using_the_view_macro() -> impl ::xcss_web::IntoView {
    xcss_web::view! { "ok" }
}

fn main() {}
