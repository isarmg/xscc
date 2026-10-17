use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const ANCHOR: &str = "\u{200b}";
const MAX_CONTEXT_UTF16: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    Text(String),
    Backspace,
    Return,
    Tab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    StaleEpoch,
    InvalidRange,
    UnsupportedEdit,
    UnsupportedCharacter,
    ContextTooLong,
}

/// A bounded local suffix, never a representation of the remote document.
pub struct KeyboardState {
    document: String,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    epoch: u64,
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardState {
    pub fn new() -> Self {
        Self {
            document: ANCHOR.into(),
            selection: 1..1,
            marked: None,
            epoch: 0,
        }
    }

    pub fn document(&self) -> &str {
        &self.document
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Also use on cancellation, focus/session changes, and failed remote sends.
    pub fn reset(&mut self) {
        self.document.clear();
        self.document.push_str(ANCHOR);
        self.selection = 1..1;
        self.marked = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn reject<T>(&mut self, error: InputError) -> Result<T, InputError> {
        self.reset();
        Err(error)
    }

    fn validate_epoch(&mut self, epoch: u64) -> Result<(), InputError> {
        if epoch != self.epoch {
            return self.reject(InputError::StaleEpoch);
        }
        Ok(())
    }

    pub fn select(&mut self, epoch: u64, range: Range<usize>) -> Result<(), InputError> {
        self.validate_epoch(epoch)?;
        if byte_range(&self.document, &range).is_none() || range.start < 1 {
            return self.reject(InputError::InvalidRange);
        }
        let valid = match &self.marked {
            Some(marked) => range.start >= marked.start && range.end <= marked.end,
            None => {
                range.end == utf16_len(&self.document) && self.committed_suffix(&range).is_some()
            }
        };
        if !valid {
            return self.reject(InputError::UnsupportedEdit);
        }
        self.selection = range;
        Ok(())
    }

    /// `selection` is relative to the newly inserted preedit text, in UTF-16.
    pub fn mark(
        &mut self,
        epoch: u64,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
    ) -> Result<Vec<Output>, InputError> {
        self.validate_epoch(epoch)?;
        let range = range.unwrap_or_else(|| self.marked.clone().unwrap_or(self.selection.clone()));
        let Some(bytes) = byte_range(&self.document, &range) else {
            return self.reject(InputError::InvalidRange);
        };
        let end = utf16_len(&self.document);
        let valid = match &self.marked {
            Some(marked) => range.start >= marked.start && range.end <= marked.end,
            None => range == (end..end),
        };
        if range.start < 1 || !valid {
            return self.reject(InputError::UnsupportedEdit);
        }
        let text_len = utf16_len(text);
        let selection = selection.unwrap_or(text_len..text_len);
        if byte_range(text, &selection).is_none() {
            return self.reject(InputError::InvalidRange);
        }
        let new_end = end - (range.end - range.start) + text_len;
        if new_end > MAX_CONTEXT_UTF16 {
            return self.reject(InputError::ContextTooLong);
        }
        let marked_start = self
            .marked
            .as_ref()
            .map_or(range.start, |marked| marked.start);
        self.document.replace_range(bytes, text);
        self.selection = range.start + selection.start..range.start + selection.end;
        self.marked = (marked_start != new_end).then_some(marked_start..new_end);
        Ok(Vec::new())
    }

    pub fn commit(
        &mut self,
        epoch: u64,
        range: Option<Range<usize>>,
        text: &str,
        unicode_supported: bool,
    ) -> Result<Vec<Output>, InputError> {
        self.validate_epoch(epoch)?;
        let range = range.unwrap_or_else(|| self.marked.clone().unwrap_or(self.selection.clone()));
        let Some(bytes) = byte_range(&self.document, &range) else {
            return self.reject(InputError::InvalidRange);
        };
        if range == (0..1) && self.document == ANCHOR && text.is_empty() {
            self.selection = 1..1;
            return Ok(vec![Output::Backspace]);
        }
        if range.start < 1 {
            return self.reject(InputError::UnsupportedEdit);
        }
        if let Some(marked) = self.marked.clone() {
            if range.start < marked.start || range.end > marked.end {
                return self.reject(InputError::UnsupportedEdit);
            }
            // An IME deleting part of its composition has not committed the remainder.
            if text.is_empty() {
                self.document.replace_range(bytes, "");
                let end = utf16_len(&self.document);
                self.marked = (marked.start != end).then_some(marked.start..end);
                self.selection = range.start..range.start;
                return Ok(Vec::new());
            }
            if range != marked {
                return self.reject(InputError::UnsupportedEdit);
            }
        }
        let mut outputs = match split_output(text, unicode_supported) {
            Ok(outputs) => outputs,
            Err(error) => return self.reject(error),
        };
        if self.marked.is_none() {
            let Some(deleted) = self.committed_suffix(&range) else {
                return self.reject(InputError::UnsupportedEdit);
            };
            outputs.splice(0..0, std::iter::repeat_n(Output::Backspace, deleted));
        }
        self.document.replace_range(bytes, text);
        self.marked = None;
        let end = utf16_len(&self.document);
        self.selection = end..end;
        if end > MAX_CONTEXT_UTF16
            || outputs
                .iter()
                .any(|output| matches!(output, Output::Return | Output::Tab))
        {
            self.reset();
        }
        Ok(outputs)
    }

    pub fn unmark(
        &mut self,
        epoch: u64,
        unicode_supported: bool,
    ) -> Result<Vec<Output>, InputError> {
        self.validate_epoch(epoch)?;
        let Some(marked) = self.marked.clone() else {
            return Ok(Vec::new());
        };
        let Some(bytes) = byte_range(&self.document, &marked) else {
            return self.reject(InputError::InvalidRange);
        };
        let text = self.document[bytes].to_owned();
        self.commit(epoch, Some(marked), &text, unicode_supported)
    }

    fn committed_suffix(&self, range: &Range<usize>) -> Option<usize> {
        if range.start < 1 || range.end != utf16_len(&self.document) {
            return None;
        }
        let bytes = byte_range(&self.document, range)?;
        let tail = &self.document[ANCHOR.len()..];
        let start = bytes.start.checked_sub(ANCHOR.len())?;
        if start != tail.len() && !tail.grapheme_indices(true).any(|(index, _)| index == start) {
            return None;
        }
        Some(tail[start..].graphemes(true).count())
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn byte_offset(text: &str, offset: usize) -> Option<usize> {
    let mut position = 0;
    for (byte, character) in text.char_indices() {
        if position == offset {
            return Some(byte);
        }
        position += character.len_utf16();
    }
    (position == offset).then_some(text.len())
}

fn byte_range(text: &str, range: &Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(byte_offset(text, range.start)?..byte_offset(text, range.end)?)
}

fn split_output(text: &str, unicode_supported: bool) -> Result<Vec<Output>, InputError> {
    let mut output = Vec::new();
    let mut buffer = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        let special = match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                Some(Output::Return)
            }
            '\n' => Some(Output::Return),
            '\t' => Some(Output::Tab),
            c if c.is_control() || (!unicode_supported && !c.is_ascii()) => {
                return Err(InputError::UnsupportedCharacter);
            }
            _ => None,
        };
        if let Some(special) = special {
            if !buffer.is_empty() {
                output.push(Output::Text(std::mem::take(&mut buffer)));
            }
            output.push(special);
        } else {
            buffer.push(character);
        }
    }
    if !buffer.is_empty() {
        output.push(Output::Text(buffer));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_is_local_until_final_commit() {
        let mut state = KeyboardState::new();
        assert!(state.mark(0, None, "拼音", None).unwrap().is_empty());
        assert!(state.commit(0, Some(2..3), "", true).unwrap().is_empty());
        assert_eq!(state.document(), "\u{200b}拼");
        assert_eq!(
            state.unmark(0, true).unwrap(),
            vec![Output::Text("拼".into())]
        );
        assert!(state.unmark(0, true).unwrap().is_empty());
        state.mark(0, None, "候选", None).unwrap();
        state.reset();
        assert!(state.unmark(state.epoch(), true).unwrap().is_empty());
        let epoch = state.epoch();
        state.mark(epoch, None, "候选", None).unwrap();
        assert_eq!(
            state.commit(epoch, None, "选", true).unwrap(),
            vec![Output::Text("选".into())]
        );
        assert!(state.unmark(epoch, true).unwrap().is_empty());
    }

    #[test]
    fn synthetic_anchor_is_positional_and_protected() {
        let mut state = KeyboardState::new();
        assert_eq!(state.selection(), 1..1);
        assert_eq!(
            state.commit(0, Some(0..1), "", true).unwrap(),
            vec![Output::Backspace]
        );
        assert_eq!(state.document(), ANCHOR);
        assert_eq!(
            state.commit(0, None, "a\u{200b}", true).unwrap(),
            vec![Output::Text("a\u{200b}".into())]
        );
        assert_eq!(state.document(), "\u{200b}a\u{200b}");
        assert!(state.select(0, 0..3).is_err());
        assert_eq!(state.document(), ANCHOR);
        let epoch = state.epoch();
        assert!(state.commit(epoch, Some(0..1), "first", true).is_err());
        assert_eq!(state.document(), ANCHOR);
    }

    #[test]
    fn graphemes_controls_capabilities_and_epochs() {
        let mut state = KeyboardState::new();
        let emoji = "👩‍💻";
        state.commit(0, None, emoji, true).unwrap();
        let end = utf16_len(state.document());
        assert_eq!(
            state.commit(0, Some(1..end), "", true).unwrap(),
            vec![Output::Backspace]
        );
        state.commit(0, None, emoji, true).unwrap();
        assert!(state.commit(0, Some(3..6), "", true).is_err());
        let epoch = state.epoch();
        assert_eq!(
            state.commit(epoch, None, "a\r\nb\tc", false).unwrap(),
            vec![
                Output::Text("a".into()),
                Output::Return,
                Output::Text("b".into()),
                Output::Tab,
                Output::Text("c".into())
            ]
        );
        let epoch = state.epoch();
        assert!(state.commit(epoch, None, "汉", false).is_err());
        assert_eq!(state.document(), ANCHOR);
        assert!(state.commit(epoch, None, "late", true).is_err());
        let epoch = state.epoch();
        state.commit(epoch, None, "sent", true).unwrap();
        state.reset(); // The caller reports a failed send.
        assert_eq!(state.selection(), 1..1);
        assert!(state.commit(epoch, None, "late", true).is_err());
        let epoch = state.epoch();
        assert!(state.commit(epoch, None, "\u{7f}", true).is_err());
        let epoch = state.epoch();
        state.commit(epoch, None, emoji, true).unwrap();
        assert!(state.select(epoch, 2..2).is_err());
    }
}
