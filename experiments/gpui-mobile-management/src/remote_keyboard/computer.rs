use super::{Command, KeyboardEvent, RemoteKeyboard};
use crate::keyboard_action::{Shortcut, chords::Modifier};
use base::message_proto::{ControlKey, KeyEvent};
use gpui::{Context, Div, Role, Stateful, Window, div, prelude::*, px, rgb};

pub(super) const BACKGROUND: u32 = 0x111214;
pub(super) const KEY: u32 = 0x292a30;
pub(super) const FOREGROUND: u32 = 0xe7e7eb;
pub(super) const ACCENT: u32 = 0x4d88fa;

#[derive(Clone, Copy)]
enum Key {
    Character(char),
    Control(ControlKey),
}

impl RemoteKeyboard {
    fn press_computer_key(
        &mut self,
        key: Key,
        modifiers: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.control.enabled.get() {
            return;
        }
        if matches!(key, Key::Control(ControlKey::CapsLock)) {
            self.control.chords.borrow_mut().toggle_caps();
            cx.notify();
            return;
        }
        let mut event = KeyEvent {
            press: true,
            ..Default::default()
        };
        let mut state = self.control.chords.borrow_mut();
        let modifiers = match key {
            Key::Character(value) => {
                let (character, modifiers) = modifiers
                    .map_or_else(|| state.printable(value), |modifiers| (value, modifiers));
                event.set_chr(character as u32);
                modifiers
            }
            Key::Control(key) => {
                event.set_control_key(key);
                modifiers.unwrap_or_else(|| state.take())
            }
        };
        if modifiers != 0 && !self.control.shortcuts.get() {
            return;
        }
        event.modifiers = [
            (Modifier::Control, ControlKey::Control),
            (Modifier::Shift, ControlKey::Shift),
            (Modifier::Alt, ControlKey::Alt),
            (Modifier::Meta, ControlKey::Meta),
        ]
        .into_iter()
        .filter_map(|(flag, key)| (modifiers & flag as u8 != 0).then_some(key.into()))
        .collect();
        drop(state);
        self.control.state.borrow_mut().reset();
        cx.emit(KeyboardEvent {
            epoch: self.control.epoch(),
            commands: vec![Command::Chord(event)],
        });
        self.changed(window, cx);
    }

    fn computer_key(
        &self,
        label: &str,
        key: Key,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let enabled = self.control.enabled.get();
        let selected = matches!(key, Key::Control(ControlKey::CapsLock))
            && self.control.chords.borrow().caps();
        let label = if let Key::Character(value) = key {
            if value.is_ascii_alphabetic() {
                self.control.chords.borrow().character(value).to_string()
            } else {
                label.to_owned()
            }
        } else {
            label.to_owned()
        };
        button(
            format!("computer-key-{label}"),
            &label,
            enabled,
            selected,
            height,
        )
        .flex_1()
        .min_w_0()
        .on_click(cx.listener(move |view, _, window, cx| {
            view.press_computer_key(key, None, window, cx);
        }))
    }

    pub(super) fn computer_keyboard(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let landscape =
            window.fully_visible_bounds().size.width >= window.fully_visible_bounds().size.height;
        let height = if landscape { 30. } else { 38. };
        let enabled = self.control.enabled.get();
        let modifiers_enabled = enabled && self.control.shortcuts.get();
        let mut modifiers = div().flex().gap_1().items_center().child(
            button(
                "computer-combination-mode",
                if self.control.chords.borrow().combination() {
                    "☑ 组合键"
                } else {
                    "☐ 组合键"
                },
                modifiers_enabled,
                false,
                height,
            )
            .px_2()
            .on_click(cx.listener(|view, _, _, cx| {
                if view.control.enabled.get() && view.control.shortcuts.get() {
                    view.control.chords.borrow_mut().toggle_combination();
                    cx.notify();
                }
            })),
        );
        for (label, modifier) in [
            ("Ctrl", Modifier::Control),
            ("Shift", Modifier::Shift),
            ("Alt", Modifier::Alt),
            ("Win", Modifier::Meta),
        ] {
            modifiers = modifiers.child(
                button(
                    format!("computer-modifier-{label}"),
                    label,
                    modifiers_enabled,
                    self.control.chords.borrow().selected(modifier),
                    height,
                )
                .flex_1()
                .min_w_0()
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.control.enabled.get() && view.control.shortcuts.get() {
                        view.control.chords.borrow_mut().toggle(modifier);
                        cx.notify();
                    }
                })),
            );
        }
        let rows = if self.computer_second_page {
            if landscape {
                symbol_rows()
            } else {
                portrait_symbol_rows()
            }
        } else {
            let mut rows = letter_rows();
            if !landscape {
                rows.insert(0, symbol_rows().remove(0));
            }
            rows
        };
        let columns = if !landscape && self.computer_second_page {
            6
        } else {
            10
        };
        let mut body = div().flex().flex_col().gap_1();
        for row in rows {
            for keys in row.chunks(columns) {
                let mut line = div().flex().gap_1();
                for (label, key) in keys {
                    line = line.child(self.computer_key(label, *key, height, cx));
                }
                body = body.child(line);
            }
        }
        let mut pages = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h(px(20.));
        for second in [false, true] {
            pages = pages.child(
                div()
                    .id(if second {
                        "computer-page-symbols"
                    } else {
                        "computer-page-letters"
                    })
                    .role(Role::Button)
                    .aria_label(if second {
                        "符号与功能键"
                    } else {
                        "字母与数字键"
                    })
                    .w(px(32.))
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().w(px(24.)).h(px(4.)).rounded_full().bg(rgb(
                        if self.computer_second_page == second {
                            FOREGROUND
                        } else {
                            0x66676b
                        },
                    )))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.computer_second_page = second;
                        cx.notify();
                    })),
            );
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(modifiers.flex_shrink_0())
            .child(
                div()
                    .id("computer-key-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(pages.flex_shrink_0())
    }

    pub(super) fn shortcut_keyboard(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let columns = if window.fully_visible_bounds().size.width
            >= window.fully_visible_bounds().size.height
        {
            6
        } else {
            3
        };
        let enabled = self.control.enabled.get();
        let supported = enabled && self.control.shortcuts.get();
        let mut grid = div().grid().grid_cols(columns).gap_2();
        for (label, detail, key, modifiers) in [
            (
                "Caps Lock",
                "大小写锁定",
                Key::Control(ControlKey::CapsLock),
                0,
            ),
            (
                "Ctrl+C",
                "复制",
                Key::Character('c'),
                Modifier::Control as u8,
            ),
            (
                "Ctrl+V",
                "粘贴",
                Key::Character('v'),
                Modifier::Control as u8,
            ),
            (
                "Ctrl+X",
                "剪切",
                Key::Character('x'),
                Modifier::Control as u8,
            ),
            (
                "Ctrl+A",
                "全选",
                Key::Character('a'),
                Modifier::Control as u8,
            ),
            (
                "Ctrl+Z",
                "撤销",
                Key::Character('z'),
                Modifier::Control as u8,
            ),
            (
                "Ctrl+S",
                "保存",
                Key::Character('s'),
                Modifier::Control as u8,
            ),
            ("Win", "开始", Key::Control(ControlKey::Meta), 0),
            (
                "Win+D",
                "显示桌面",
                Key::Character('d'),
                Modifier::Meta as u8,
            ),
            (
                "Win+Tab",
                "切换窗口",
                Key::Control(ControlKey::Tab),
                Modifier::Meta as u8,
            ),
            (
                "Win+L",
                "锁定屏幕",
                Key::Character('l'),
                Modifier::Meta as u8,
            ),
        ] {
            let local = matches!(key, Key::Control(ControlKey::CapsLock));
            grid = grid.child(
                button(
                    format!("computer-shortcut-{label}"),
                    "",
                    if local { enabled } else { supported },
                    local && self.control.chords.borrow().caps(),
                    62.,
                )
                .aria_label(format!("{label} {detail}"))
                .flex_col()
                .gap_1()
                .child(label)
                .child(div().text_size(px(12.)).child(detail))
                .on_click(cx.listener(move |view, _, window, cx| {
                    if !view.control.enabled.get() || !local && !view.control.shortcuts.get() {
                        return;
                    }
                    view.control.chords.borrow_mut().clear();
                    match key {
                        Key::Character('a') => view.shortcut(Shortcut::SelectAll, window, cx),
                        Key::Character('z') => view.shortcut(Shortcut::Undo, window, cx),
                        Key::Character('s') => view.shortcut(Shortcut::Save, window, cx),
                        _ => view.press_computer_key(key, Some(modifiers), window, cx),
                    }
                })),
            );
        }
        grid
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    enabled: bool,
    selected: bool,
    height: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.to_owned())
        .h(px(height))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rgb(if selected { ACCENT } else { KEY }))
        .text_color(rgb(FOREGROUND))
        .text_size(px(14.))
        .opacity(if enabled { 1. } else { 0.4 })
        .child(label.to_owned())
        .when(enabled, |key| {
            key.cursor_pointer().active(|style| style.bg(rgb(0x41434c)))
        })
}

fn letter_rows() -> Vec<Vec<(&'static str, Key)>> {
    use ControlKey::*;
    use Key::{Character as C, Control as K};
    vec![
        vec![
            ("1", C('1')),
            ("2", C('2')),
            ("3", C('3')),
            ("4", C('4')),
            ("5", C('5')),
            ("6", C('6')),
            ("7", C('7')),
            ("8", C('8')),
            ("9", C('9')),
            ("0", C('0')),
        ],
        vec![
            ("Q", C('q')),
            ("W", C('w')),
            ("E", C('e')),
            ("R", C('r')),
            ("T", C('t')),
            ("Y", C('y')),
            ("U", C('u')),
            ("I", C('i')),
            ("O", C('o')),
            ("P", C('p')),
        ],
        vec![
            ("A", C('a')),
            ("S", C('s')),
            ("D", C('d')),
            ("F", C('f')),
            ("G", C('g')),
            ("H", C('h')),
            ("J", C('j')),
            ("K", C('k')),
            ("L", C('l')),
            ("⌫", K(Backspace)),
        ],
        vec![
            ("Z", C('z')),
            ("X", C('x')),
            ("C", C('c')),
            ("V", C('v')),
            ("B", C('b')),
            ("N", C('n')),
            ("M", C('m')),
            ("Space", K(Space)),
            ("Enter", K(Return)),
        ],
    ]
}

fn symbol_rows() -> Vec<Vec<(&'static str, Key)>> {
    use ControlKey::*;
    use Key::{Character as C, Control as K};
    vec![
        vec![
            ("_ −", C('-')),
            ("+ =", C('=')),
            ("{ [", C('[')),
            ("} ]", C(']')),
            ("|", C('\\')),
            (": ;", C(';')),
            ("\" '", C('\'')),
            ("< ,", C(',')),
            ("> .", C('.')),
            ("? /", C('/')),
        ],
        vec![
            ("F1", K(F1)),
            ("F2", K(F2)),
            ("F3", K(F3)),
            ("F4", K(F4)),
            ("PrtScr", K(Snapshot)),
            ("ScrLk", K(Scroll)),
            ("Pause", K(Pause)),
            ("Esc", K(Escape)),
            ("Tab", K(Tab)),
            ("~ `", C('`')),
        ],
        vec![
            ("F5", K(F5)),
            ("F6", K(F6)),
            ("F7", K(F7)),
            ("F8", K(F8)),
            ("Ins", K(Insert)),
            ("Home", K(Home)),
            ("PgUp", K(PageUp)),
            ("Caps", K(CapsLock)),
            ("↑", K(UpArrow)),
        ],
        vec![
            ("F9", K(F9)),
            ("F10", K(F10)),
            ("F11", K(F11)),
            ("F12", K(F12)),
            ("Del", K(Delete)),
            ("End", K(End)),
            ("PgDn", K(PageDown)),
            ("←", K(LeftArrow)),
            ("↓", K(DownArrow)),
            ("→", K(RightArrow)),
        ],
    ]
}

fn portrait_symbol_rows() -> Vec<Vec<(&'static str, Key)>> {
    use ControlKey::*;
    use Key::{Character as C, Control as K};
    vec![
        vec![
            ("Esc", K(Escape)),
            ("Tab", K(Tab)),
            ("~ `", C('`')),
            ("PrtScr", K(Snapshot)),
            ("ScrLk", K(Scroll)),
            ("Pause", K(Pause)),
        ],
        vec![
            ("F1", K(F1)),
            ("F2", K(F2)),
            ("F3", K(F3)),
            ("Ins", K(Insert)),
            ("Home", K(Home)),
            ("PgUp", K(PageUp)),
        ],
        vec![
            ("F4", K(F4)),
            ("F5", K(F5)),
            ("F6", K(F6)),
            ("Del", K(Delete)),
            ("End", K(End)),
            ("PgDn", K(PageDown)),
        ],
        vec![
            ("F7", K(F7)),
            ("F8", K(F8)),
            ("F9", K(F9)),
            ("Caps", K(CapsLock)),
            ("↑", K(UpArrow)),
        ],
        vec![
            ("F10", K(F10)),
            ("F11", K(F11)),
            ("F12", K(F12)),
            ("←", K(LeftArrow)),
            ("↓", K(DownArrow)),
            ("→", K(RightArrow)),
        ],
    ]
}
