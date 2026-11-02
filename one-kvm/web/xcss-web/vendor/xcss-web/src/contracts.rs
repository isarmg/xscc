use serde::{Deserialize, Serialize};

pub const LOGIN_PATH: &str = "/api/v1/auth/login";
pub const SESSION_PATH: &str = "/api/v1/auth/session";
pub const LOGOUT_PATH: &str = "/api/v1/auth/logout";
pub const ACCOUNT_PATH: &str = "/api/v1/platform/administrators/self";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdministratorSession {
    pub authenticated: bool,
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub csrf_token: String,
}

impl AdministratorSession {
    pub fn validate(&self) -> bool {
        self.authenticated
            && self.role == "admin"
            && safe_request_id(&self.user_id)
            && (3..=64).contains(&self.username.len())
            && self
                .username
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            && self
                .username
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && self
                .username
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && self.csrf_token.len() == 43
            && self
                .csrf_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && self
                .csrf_token
                .as_bytes()
                .last()
                .is_some_and(|b| b"AEIMQUYcgkosw048".contains(b))
    }
}

pub fn valid_username_candidate(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

pub fn valid_password(value: &str) -> bool {
    (12..=1024).contains(&value.len()) && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

pub fn valid_instance_name(value: &str, maximum: usize) -> bool {
    (1..=32).contains(&maximum)
        && !value.trim().is_empty()
        && value.trim().chars().count() <= maximum
        && !value
            .chars()
            .any(|c| c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c))
}

pub fn safe_request_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_keep_utf8_byte_policy_and_unicode_names() {
        assert!(valid_password("密码密码密码"));
        assert!(!valid_password("short"));
        assert!(!valid_password("abcdefghijkl\n"));
        assert!(valid_instance_name(&"😀".repeat(32), 32));
        assert!(!valid_instance_name(&"😀".repeat(33), 32));
        assert!(!valid_instance_name("name\u{85}", 32));
    }

    #[test]
    fn unknown_session_fields_and_nonadmin_roles_are_rejected() {
        let mut value = serde_json::json!({"authenticated":true,"user_id":"123e4567-e89b-12d3-a456-426614174000","username":"admin","role":"admin","csrf_token":"A".repeat(43)});
        assert!(
            serde_json::from_value::<AdministratorSession>(value.clone())
                .unwrap()
                .validate()
        );
        value["role"] = serde_json::json!("viewer");
        assert!(
            !serde_json::from_value::<AdministratorSession>(value.clone())
                .unwrap()
                .validate()
        );
        value["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<AdministratorSession>(value).is_err());
    }
}
