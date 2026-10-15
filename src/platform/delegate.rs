use cocoa::base::id;
use hbb_common::log;
use objc::runtime::{Object, Sel, BOOL, NO};

// Customize the service opening logic.
#[no_mangle]
fn service_should_handle_reopen(
    _obj: &Object,
    _sel: Sel,
    _sender: id,
    _has_visible_windows: BOOL,
) -> BOOL {
    log::debug!("Invoking the main rustdesk process");
    std::thread::spawn(move || crate::handle_url_scheme("".to_string()));
    // Prevent default logic.
    NO
}
