//! GameTextInput snapshots use Java Editable's UTF-16 offsets, even though the
//! android-activity binding exposes their text as a Rust UTF-8 String.
use crate::{text_state::State as TextState, window::WindowState};
use android_activity::input::{ImeOptions, InputType, TextInputAction, TextInputState, TextSpan};
use gpui::{Autocapitalize, FocusId, PlatformInputHandler, TextInputConfiguration};

#[derive(Default)]
pub(crate) struct ImeState {
    focus: Option<FocusId>,
    sent: Option<TextState>,
    configuration: Option<TextInputConfiguration>,
    pub requested: bool,
    seen_visible: bool,
    generation: u64,
}

fn snapshot(handler: &mut PlatformInputHandler) -> Option<(FocusId, TextState)> {
    if !handler.query_accepts_focused_text_input() {
        return None;
    }
    let focus = handler.focus_id()?;
    let length = handler.text_length_utf16()?;
    let text = handler.text_for_range(0..length, &mut None)?;
    let selected = handler.selected_text_range(true)?;
    let range = if selected.reversed {
        selected.range.end..selected.range.start
    } else {
        selected.range
    };
    Some((
        focus,
        TextState::new(text, range, handler.marked_text_range()),
    ))
}

impl WindowState {
    /// Called outside GPUI update/paint transactions. Handler calls may enter GPUI.
    pub(crate) fn sync_ime(&self) {
        if !self.visible.get() {
            self.dismiss_keyboard();
            return;
        }
        let current = self.input_handler.with(snapshot).flatten();
        let configuration = self.ime_configuration.borrow().clone();
        let mut ime = self.ime.borrow_mut();
        let Some((focus, current)) = current else {
            if ime.focus.take().is_some() {
                ime.generation = crate::ime_bridge::Bridge::next_generation();
                self.ime_bridge.hide(ime.generation);
                ime.sent = None;
                ime.requested = false;
            }
            return;
        };
        let show_requested = self.ime_show.replace(false);
        let changed_session = ime.focus != Some(focus)
            || ime.configuration.as_ref() != Some(&configuration)
            || show_requested
            || ime.sent.as_ref() != Some(&current);
        if changed_session {
            let (kind, action, options) = editor_info(&configuration);
            let action: i32 = action.into();
            ime.generation = crate::ime_bridge::Bridge::next_generation();
            ime.requested |= show_requested;
            self.ime_bridge.configure(
                ime.generation,
                kind.bits(),
                action,
                options.bits(),
                native_state(&current),
                ime.requested,
            );
            ime.focus = Some(focus);
            ime.configuration = Some(configuration);
            ime.sent = Some(current);
        }
    }

    pub(crate) fn read_ime_events(&self) {
        for event in self.ime_bridge.drain() {
            if event.generation != self.ime.borrow().generation {
                continue;
            }
            match event.kind {
                crate::ime_bridge::EventKind::Text(state) => {
                    self.handle_text_event(&state);
                }
                crate::ime_bridge::EventKind::Action(action) => {
                    if self.accepts_text_action() {
                        let action = match action {
                            2 => TextInputAction::Go,
                            3 => TextInputAction::Search,
                            4 => TextInputAction::Send,
                            5 => TextInputAction::Next,
                            6 => TextInputAction::Done,
                            7 => TextInputAction::Previous,
                            1 => TextInputAction::None,
                            _ => TextInputAction::Unspecified,
                        };
                        for event in crate::input::text_action(action) {
                            self.input.with(|f| f(event));
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn accepts_text_action(&self) -> bool {
        if !self.visible.get() || !self.active.get() {
            return false;
        }
        let focus = self.ime.borrow().focus;
        self.input_handler
            .with(|handler| {
                handler.query_accepts_focused_text_input()
                    && handler.focus_id().is_some()
                    && handler.focus_id() == focus
            })
            .unwrap_or(false)
    }

    pub(crate) fn handle_text_event(&self, incoming: &TextInputState) -> bool {
        if !self.visible.get() || !self.active.get() {
            return false;
        }
        let (focus, baseline) = {
            let ime = self.ime.borrow();
            (ime.focus, ime.sent.clone())
        };
        let incoming = TextState::new(
            incoming.text.clone(),
            incoming.selection.start..incoming.selection.end,
            incoming.compose_region.as_ref().map(|r| r.start..r.end),
        );
        let handled = self
            .input_handler
            .with(|handler| {
                let Some((current_focus, current)) = snapshot(handler) else {
                    return false;
                };
                if Some(current_focus) != focus || baseline.as_ref() != Some(&current) {
                    return false;
                }
                if current == incoming {
                    return true;
                }
                if let Some(edit) = current.replacement(&incoming) {
                    handler.replace_text_in_range(Some(edit.range), &edit.text);
                    // Rejected/truncated edits invalidate incoming composing offsets.
                    if snapshot(handler).is_none_or(|(_, accepted)| accepted.text != incoming.text)
                    {
                        return true;
                    }
                }
                if let (Some(range), Some(text)) = (&incoming.composing, incoming.composing_text())
                {
                    handler.replace_and_mark_text_in_range(Some(range.clone()), text, None);
                } else {
                    handler.unmark_text();
                }
                handler.set_selected_text_range(incoming.selection.clone());
                true
            })
            .unwrap_or(false);
        if handled {
            // Treat this as the native state; sync will correct rejected/truncated
            // edits using the actual handler state, without echoing accepted edits.
            self.ime.borrow_mut().sent = Some(incoming);
            self.schedule_frame();
        }
        handled
    }

    pub(crate) fn ime_visibility_changed(&self, visible: bool) {
        let mut ime = self.ime.borrow_mut();
        if visible {
            ime.seen_visible = true;
        } else if ime.seen_visible {
            ime.requested = false;
            ime.seen_visible = false;
        }
    }

    pub(crate) fn dismiss_keyboard(&self) -> bool {
        self.ime_show.set(false);
        let mut ime = self.ime.borrow_mut();
        let requested = ime.requested
            || self
                .native_insets
                .get()
                .is_some_and(|insets| insets.ime_visible());
        if ime.requested || ime.focus.is_some() {
            ime.generation = crate::ime_bridge::Bridge::next_generation();
            self.ime_bridge.hide(ime.generation);
            ime.focus = None;
            ime.sent = None;
        }
        ime.requested = false;
        requested
    }
}
fn native_state(state: &TextState) -> TextInputState {
    TextInputState {
        text: state.text.clone(),
        selection: TextSpan {
            start: state.selection.start,
            end: state.selection.end,
        },
        compose_region: state.composing.as_ref().map(|r| TextSpan {
            start: r.start,
            end: r.end,
        }),
    }
}
fn editor_info(config: &TextInputConfiguration) -> (InputType, TextInputAction, ImeOptions) {
    let mut kind = InputType::TYPE_CLASS_TEXT;
    let mut options = ImeOptions::IME_FLAG_NO_FULLSCREEN;
    if config.secure {
        kind |= InputType::TYPE_TEXT_VARIATION_PASSWORD | InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        options |= ImeOptions::IME_FLAG_NO_PERSONALIZED_LEARNING;
    } else {
        if !config.suggestions {
            kind |= InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        }
        if config.autocorrect {
            kind |= InputType::TYPE_TEXT_FLAG_AUTO_CORRECT;
        }
        kind |= match config.autocapitalize {
            Autocapitalize::None => InputType::empty(),
            Autocapitalize::Words => InputType::TYPE_TEXT_FLAG_CAP_WORDS,
            Autocapitalize::Sentences => InputType::TYPE_TEXT_FLAG_CAP_SENTENCES,
            Autocapitalize::Characters => InputType::TYPE_TEXT_FLAG_CAP_CHARACTERS,
        };
    }
    let action = match config.input_action {
        gpui::TextInputAction::Unspecified => TextInputAction::Unspecified,
        gpui::TextInputAction::Enter => TextInputAction::None,
        gpui::TextInputAction::Done => TextInputAction::Done,
        gpui::TextInputAction::Go => TextInputAction::Go,
        gpui::TextInputAction::Next => TextInputAction::Next,
        gpui::TextInputAction::Previous => TextInputAction::Previous,
        gpui::TextInputAction::Search => TextInputAction::Search,
        gpui::TextInputAction::Send => TextInputAction::Send,
    };
    (kind, action, options)
}
