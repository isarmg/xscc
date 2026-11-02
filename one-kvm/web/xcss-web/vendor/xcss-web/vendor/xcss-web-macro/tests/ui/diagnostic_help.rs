use xcss_web_macro::{component, island, slot, view};

fn invalid_view() {
    let _ = view! { class = "scope" <div/> };
}

#[component(unsupported)]
fn invalid_component_argument() -> impl xcss_web::prelude::IntoView {}

#[island(unsupported)]
fn invalid_island_argument() -> impl xcss_web::prelude::IntoView {}

#[slot(unsupported)]
struct InvalidSlotArgument;

#[component]
fn invalid_option(
    #[prop(strip_option, into)] value: bool,
) -> impl xcss_web::prelude::IntoView {
    value.to_string()
}

fn main() {}
