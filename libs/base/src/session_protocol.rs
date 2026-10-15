use crate::message_proto::{message, misc, Message};

pub const LOGIN_MSG_PASSWORD_EMPTY: &str = "Empty Password";
pub const LOGIN_MSG_PASSWORD_WRONG: &str = "Wrong Password";
pub const LOGIN_MSG_2FA_WRONG: &str = "Wrong 2FA Code";
pub const REQUIRE_2FA: &str = "2FA Required";
pub const LOGIN_MSG_NO_PASSWORD_ACCESS: &str = "No Password Access";
pub const LOGIN_MSG_OFFLINE: &str = "Offline";
pub const LOGIN_SCREEN_WAYLAND: &str = "Wayland login screen is not supported";
pub const SCRAP_UBUNTU_HIGHER_REQUIRED: &str = "ubuntu-21-04-required";
pub const SCRAP_XDP_PORTAL_UNAVAILABLE: &str = "xdp-portal-unavailable";

pub mod input {
    pub const MOUSE_TYPE_MOVE: i32 = 0;
    pub const MOUSE_TYPE_DOWN: i32 = 1;
    pub const MOUSE_TYPE_UP: i32 = 2;
    pub const MOUSE_TYPE_WHEEL: i32 = 3;
    pub const MOUSE_TYPE_TRACKPAD: i32 = 4;
    /// Relative mouse movement type for gaming/3D applications.
    /// This type sends delta (dx, dy) values instead of absolute coordinates.
    /// NOTE: This is only supported by the Flutter client. The Sciter client (deprecated)
    /// does not support relative mouse mode due to:
    /// 1. Fixed send_mouse() function signature that doesn't allow type differentiation
    /// 2. Lack of pointer lock API in Sciter/TIS
    /// 3. No OS cursor control (hide/show/clip) FFI bindings in Sciter UI
    pub const MOUSE_TYPE_MOVE_RELATIVE: i32 = 5;

    /// Mask to extract the mouse event type from the mask field.
    /// The lower 3 bits contain the event type (MOUSE_TYPE_*), giving a valid range of 0-7.
    /// Currently defined types use values 0-5; values 6 and 7 are reserved for future use.
    pub const MOUSE_TYPE_MASK: i32 = 0x7;

    pub const MOUSE_BUTTON_LEFT: i32 = 0x01;
    pub const MOUSE_BUTTON_RIGHT: i32 = 0x02;
    pub const MOUSE_BUTTON_WHEEL: i32 = 0x04;
    pub const MOUSE_BUTTON_BACK: i32 = 0x08;
    pub const MOUSE_BUTTON_FORWARD: i32 = 0x10;
}

/// Recognize retired wire messages so both endpoints reject them before authorization.
/// Keep their protobuf tags allocated so removed commands cannot be reinterpreted.
pub fn is_legacy_role_switch_message(msg: &Message) -> bool {
    match msg.union.as_ref() {
        Some(message::Union::SwitchSidesResponse(_)) => true,
        Some(message::Union::Misc(misc)) => matches!(
            misc.union.as_ref(),
            Some(misc::Union::SwitchSidesRequest(_) | misc::Union::SwitchBack(_))
        ),
        _ => false,
    }
}

#[cfg(test)]
mod retired_role_switch_tests {
    use super::*;
    use crate::message_proto::{
        LoginRequest, Misc, SwitchBack, SwitchSidesRequest, SwitchSidesResponse, TestDelay,
    };
    use protobuf::Message as _;

    #[test]
    fn retired_wire_tags_are_rejected_and_login_is_preserved() {
        // Retired messages still decode, but none may reach normal login processing.
        let mut request = Message::new();
        let mut misc = Misc::new();
        misc.set_switch_sides_request(SwitchSidesRequest::default());
        request.set_misc(misc);
        let mut back = Message::new();
        let mut misc = Misc::new();
        misc.set_switch_back(SwitchBack::default());
        back.set_misc(misc);
        let mut response = Message::new();
        response.set_switch_sides_response(SwitchSidesResponse::default());
        for (message, wire) in [
            (request, vec![0x9a, 0x01, 0x03, 0xaa, 0x01, 0x00]),
            (back, vec![0x9a, 0x01, 0x03, 0xb2, 0x01, 0x00]),
            (response, vec![0xb2, 0x01, 0x00]),
        ] {
            assert_eq!(message.write_to_bytes().unwrap(), wire);
            let decoded = Message::parse_from_bytes(&wire).unwrap();
            assert!(is_legacy_role_switch_message(&decoded));
        }
        let mut login = Message::new();
        login.set_login_request(LoginRequest::default());
        assert!(!is_legacy_role_switch_message(&login));
        let mut delay = Message::new();
        delay.set_test_delay(TestDelay::default());
        assert!(!is_legacy_role_switch_message(&delay));
    }
}
