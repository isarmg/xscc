pub fn usage(code: &str) -> Option<u8> {
    if let Some(letter) = code.strip_prefix("Key") {
        let byte = *letter.as_bytes().first()?;
        if letter.len() == 1 && byte.is_ascii_uppercase() {
            return Some(byte - b'A' + 4);
        }
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return match digit {
            "0" => Some(39),
            value if value.len() == 1 => value
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .map(|n| n + 29),
            _ => None,
        };
    }
    if let Some(function) = code.strip_prefix('F')
        && let Ok(number) = function.parse::<u8>()
    {
        if (1..=12).contains(&number) {
            return Some(number + 57);
        }
        if (13..=24).contains(&number) {
            return Some(number + 91);
        }
    }
    Some(match code {
        "Enter" => 40,
        "Escape" => 41,
        "Backspace" => 42,
        "Tab" => 43,
        "Space" => 44,
        "Minus" => 45,
        "Equal" => 46,
        "BracketLeft" => 47,
        "BracketRight" => 48,
        "Backslash" => 49,
        "IntlBackslash" => 100,
        "Semicolon" => 51,
        "Quote" => 52,
        "Backquote" => 53,
        "Comma" => 54,
        "Period" => 55,
        "Slash" => 56,
        "CapsLock" => 57,
        "PrintScreen" => 70,
        "ScrollLock" => 71,
        "Pause" => 72,
        "Insert" => 73,
        "Home" => 74,
        "PageUp" => 75,
        "Delete" => 76,
        "End" => 77,
        "PageDown" => 78,
        "ArrowRight" => 79,
        "ArrowLeft" => 80,
        "ArrowDown" => 81,
        "ArrowUp" => 82,
        "NumLock" => 83,
        "NumpadDivide" => 84,
        "NumpadMultiply" => 85,
        "NumpadSubtract" => 86,
        "NumpadAdd" => 87,
        "NumpadEnter" => 88,
        "Numpad1" => 89,
        "Numpad2" => 90,
        "Numpad3" => 91,
        "Numpad4" => 92,
        "Numpad5" => 93,
        "Numpad6" => 94,
        "Numpad7" => 95,
        "Numpad8" => 96,
        "Numpad9" => 97,
        "Numpad0" => 98,
        "NumpadDecimal" => 99,
        "ContextMenu" => 101,
        "ControlLeft" => 224,
        "ShiftLeft" => 225,
        "AltLeft" => 226,
        "MetaLeft" => 227,
        "ControlRight" => 228,
        "ShiftRight" => 229,
        "AltRight" => 230,
        "MetaRight" => 231,
        _ => return None,
    })
}

pub fn keyboard(down: bool, key: u8, modifier: u8) -> [u8; 4] {
    [1, u8::from(!down), key, modifier]
}

pub fn mouse(kind: u8, x: i16, y: i16, button: i8) -> [u8; 7] {
    let x = x.to_le_bytes();
    let y = y.to_le_bytes();
    [2, kind, x[0], x[1], y[0], y[1], button as u8]
}

pub fn character(value: char) -> Option<(u8, u8)> {
    Some(match value {
        'a'..='z' => (value as u8 - b'a' + 4, 0),
        'A'..='Z' => (value as u8 - b'A' + 4, 2),
        '1'..='9' => (value as u8 - b'1' + 30, 0),
        '0' => (39, 0),
        ' ' => (44, 0),
        '\n' => (40, 0),
        '\t' => (43, 0),
        '-' => (45, 0),
        '_' => (45, 2),
        '=' => (46, 0),
        '+' => (46, 2),
        '[' => (47, 0),
        '{' => (47, 2),
        ']' => (48, 0),
        '}' => (48, 2),
        '\\' => (49, 0),
        '|' => (49, 2),
        ';' => (51, 0),
        ':' => (51, 2),
        '\'' => (52, 0),
        '"' => (52, 2),
        '`' => (53, 0),
        '~' => (53, 2),
        ',' => (54, 0),
        '<' => (54, 2),
        '.' => (55, 0),
        '>' => (55, 2),
        '/' => (56, 0),
        '?' => (56, 2),
        '!' => (30, 2),
        '@' => (31, 2),
        '#' => (32, 2),
        '$' => (33, 2),
        '%' => (34, 2),
        '^' => (35, 2),
        '&' => (36, 2),
        '*' => (37, 2),
        '(' => (38, 2),
        ')' => (39, 2),
        '\r' => return None,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_keys_and_binary_messages_match_hardware_protocol() {
        assert_eq!(usage("KeyA"), Some(4));
        assert_eq!(usage("Digit0"), Some(39));
        assert_eq!(usage("F12"), Some(69));
        assert_eq!(usage("NumpadEnter"), Some(88));
        assert_eq!(keyboard(true, 76, 5), [1, 0, 76, 5]);
        assert_eq!(mouse(0, -2, 300, 0), [2, 0, 254, 255, 44, 1, 0]);
        assert_eq!(character('@'), Some((31, 2)));
        assert_eq!(character('中'), None);
    }
}
