use crate::keyboard_input::{KeyboardState, Output};
use gpui::KeyDownEvent;

// Android editor actions arrive as ordinary key pairs. Only these two unmodified
// controls are accepted; printable hardware keys are never treated as committed text.
pub fn editor_action(event: &KeyDownEvent) -> Option<Output> {
    if event.is_held || event.keystroke.modifiers != gpui::Modifiers::default() {
        return None;
    }
    match event.keystroke.key.as_str() {
        "enter" => Some(Output::Return),
        "tab" => Some(Output::Tab),
        _ => None,
    }
}

pub fn finish_editor_action(
    state: &mut KeyboardState,
    action: Output,
    unicode: bool,
) -> Result<Vec<Output>, crate::keyboard_input::InputError> {
    let mut outputs = state.unmark(state.epoch(), unicode)?;
    outputs.push(action);
    state.reset();
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_action_pair_only_down_finalizes_and_emits_one_control() {
        // Same KeyDown shape as gpui_android::input::text_action; its following
        // KeyUp has no handler here and cannot produce a second command.
        let mut down = KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: "enter".into(),
                key_char: None,
                modifiers: gpui::Modifiers::default(),
            },
            is_held: false,
            prefer_character_input: false,
        };
        let mut state = KeyboardState::new();
        state.mark(state.epoch(), None, "中文", None).unwrap();
        let epoch = state.epoch();
        assert_eq!(
            finish_editor_action(&mut state, editor_action(&down).unwrap(), true).unwrap(),
            vec![Output::Text("中文".into()), Output::Return]
        );
        assert_ne!(state.epoch(), epoch);
        assert!(state.unmark(state.epoch(), true).unwrap().is_empty());
        down.keystroke.key = "tab".into();
        assert_eq!(
            finish_editor_action(&mut state, editor_action(&down).unwrap(), true).unwrap(),
            vec![Output::Tab]
        );
        down.is_held = true;
        assert!(editor_action(&down).is_none());
        down.is_held = false;
        down.keystroke.modifiers.shift = true;
        assert!(editor_action(&down).is_none());
        down.keystroke.modifiers = gpui::Modifiers::default();
        down.keystroke.key = "a".into();
        assert!(editor_action(&down).is_none());
        state.mark(state.epoch(), None, "中", None).unwrap();
        assert!(finish_editor_action(&mut state, Output::Return, false).is_err());
        assert!(state.marked().is_none());
    }
}

/// Explicit one-shot actions, not an arbitrary key/modifier injection interface.
#[derive(Clone, Copy)]
pub enum Shortcut {
    SelectAll,
    Undo,
    Save,
}
impl Shortcut {
    pub fn character(self) -> char {
        match self {
            Self::SelectAll => 'a',
            Self::Undo => 'z',
            Self::Save => 's',
        }
    }
    pub fn cancel_composition(self, state: &mut KeyboardState) {
        state.reset();
    }
}
#[cfg(test)]
mod shortcut_tests {
    use super::*;
    #[test]
    fn shortcuts_cancel_preedit_and_invalidate_old_commits() {
        for (shortcut, chr) in [
            (Shortcut::SelectAll, 'a'),
            (Shortcut::Undo, 'z'),
            (Shortcut::Save, 's'),
        ] {
            let mut state = KeyboardState::new();
            let old = state.epoch();
            state.mark(old, None, "未提交", None).unwrap();
            shortcut.cancel_composition(&mut state);
            assert_eq!(shortcut.character(), chr);
            assert_ne!(state.epoch(), old);
            assert!(state.marked().is_none());
            assert!(state.unmark(state.epoch(), true).unwrap().is_empty());
            assert!(state.commit(old, None, "迟到", true).is_err());
        }
    }
}
