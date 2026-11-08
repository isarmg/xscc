#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Control = 1,
    Shift = 2,
    Alt = 4,
    Meta = 8,
}

#[derive(Default)]
pub struct ChordState {
    combination: bool,
    selected: u8,
    caps: bool,
}

impl ChordState {
    pub fn combination(&self) -> bool {
        self.combination
    }
    pub fn toggle_combination(&mut self) {
        self.combination = !self.combination;
    }
    pub fn toggle(&mut self, modifier: Modifier) {
        self.selected ^= modifier as u8;
    }
    pub fn selected(&self, modifier: Modifier) -> bool {
        self.selected & modifier as u8 != 0
    }
    pub fn clear(&mut self) {
        self.selected = 0;
    }
    pub fn caps(&self) -> bool {
        self.caps
    }
    pub fn toggle_caps(&mut self) {
        self.caps = !self.caps;
    }
    pub fn take(&mut self) -> u8 {
        let selected = self.selected;
        if !self.combination {
            self.clear();
        }
        selected
    }
    pub fn printable(&mut self, value: char) -> (char, u8) {
        let visible = self.character(value);
        let modifiers = self.take();
        if modifiers & (Modifier::Control as u8 | Modifier::Alt as u8 | Modifier::Meta as u8) != 0 {
            (value, modifiers)
        } else {
            // Legacy Chr already contains the printable result; Shift is hotkey metadata only.
            (visible, 0)
        }
    }

    pub fn character(&self, value: char) -> char {
        if value.is_ascii_alphabetic() {
            return if self.caps ^ self.selected(Modifier::Shift) {
                value.to_ascii_uppercase()
            } else {
                value.to_ascii_lowercase()
            };
        }
        if !self.selected(Modifier::Shift) {
            return value;
        }
        match value {
            '1' => '!',
            '2' => '@',
            '3' => '#',
            '4' => '$',
            '5' => '%',
            '6' => '^',
            '7' => '&',
            '8' => '*',
            '9' => '(',
            '0' => ')',
            '-' => '_',
            '=' => '+',
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            ';' => ':',
            '\'' => '"',
            '`' => '~',
            ',' => '<',
            '.' => '>',
            '/' => '?',
            _ => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_apply_once_or_until_cleared_without_remote_holds() {
        let mut state = ChordState::default();
        state.toggle(Modifier::Control);
        state.toggle(Modifier::Shift);
        assert_eq!(state.take(), 3);
        assert_eq!(state.take(), 0);
        state.toggle_combination();
        assert!(state.combination());
        state.toggle(Modifier::Alt);
        state.toggle(Modifier::Meta);
        assert_eq!(state.take(), 12);
        assert_eq!(state.take(), 12);
        state.clear();
        assert_eq!(state.take(), 0);
    }
    #[test]
    fn printable_wire_character_has_one_source_for_shift_caps_and_hotkeys() {
        let mut state = ChordState::default();
        assert_eq!(state.printable('q'), ('q', 0));
        state.toggle_caps();
        assert!(state.caps());
        assert_eq!(state.printable('q'), ('Q', 0));
        state.toggle(Modifier::Shift);
        assert_eq!(state.printable('q'), ('q', 0));
        for (plain, shifted) in [('1', '!'), ('[', '{'), ('/', '?'), ('\\', '|')] {
            state.toggle(Modifier::Shift);
            assert_eq!(state.printable(plain), (shifted, 0));
        }
        state.toggle(Modifier::Control);
        state.toggle(Modifier::Shift);
        assert_eq!(state.printable('z'), ('z', 3));
        assert_eq!(state.printable('q'), ('Q', 0));
    }
}
