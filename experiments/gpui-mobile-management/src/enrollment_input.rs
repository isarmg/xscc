use xscc::protocol::{AUTHORIZATION_CODE_LENGTH, is_valid_authorization_code};

pub(super) fn instance_password(input: &str) -> Result<&str, String> {
    let password = input.trim();
    if password.is_empty() {
        return Err("请填写控制端实例密码。".into());
    }
    let count = password.chars().count();
    if count != AUTHORIZATION_CODE_LENGTH {
        return Err(format!(
            "实例密码应为 {AUTHORIZATION_CODE_LENGTH} 位小写英文字母或数字，当前输入 {count} 个字符。请复制服务端显示的实例密码。"
        ));
    }
    if !is_valid_authorization_code(password) {
        return Err("实例密码只能包含小写英文字母 a-z 和数字 0-9，不能包含内部空格或其他字符。请使用实例密码，不是实例名称、ID 或管理员密码。".into());
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn pasted_outer_whitespace_is_removed_without_changing_the_password() {
        assert_eq!(instance_password(PASSWORD).unwrap(), PASSWORD);
        assert_eq!(
            instance_password(&format!(" \r\n\t{PASSWORD}\u{a0} ")).unwrap(),
            PASSWORD
        );
    }

    #[test]
    fn empty_and_incomplete_passwords_get_specific_feedback() {
        assert_eq!(
            instance_password(" \r\n\t").unwrap_err(),
            "请填写控制端实例密码。"
        );
        assert!(
            instance_password(&PASSWORD[..35])
                .unwrap_err()
                .contains("当前输入 35 个字符")
        );
        assert!(
            instance_password(&format!("{PASSWORD}a"))
                .unwrap_err()
                .contains("当前输入 37 个字符")
        );
    }

    #[test]
    fn uppercase_internal_spaces_and_non_ascii_are_rejected_without_rewriting() {
        for input in [
            PASSWORD.replacen('a', "A", 1),
            PASSWORD.replacen('m', " ", 1),
            "界".repeat(36),
        ] {
            let error = instance_password(&input).unwrap_err();
            assert!(error.contains("只能包含小写英文字母 a-z 和数字 0-9"));
            assert!(!error.contains(&input));
        }
    }
}
