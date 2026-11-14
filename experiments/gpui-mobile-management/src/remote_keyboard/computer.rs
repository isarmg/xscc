use super::{Command, KeyboardEvent, RemoteKeyboard};
use crate::keyboard_action::{Shortcut, chords::Modifier};
use base::message_proto::{ControlKey, KeyEvent};
use gpui::{Context, Div, Role, Stateful, Window, div, prelude::*, px, rgb};

use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) enum PageMotion {
    #[default]
    Idle,
    Dragging(f32),
    Settling {
        from: f32,
        started: Instant,
    },
}

pub(super) const BACKGROUND: u32 = 0x111214;
pub(super) const KEY: u32 = 0x292a30;
pub(super) const FOREGROUND: u32 = 0xe7e7eb;
pub(super) const ACCENT: u32 = 0x4d88fa;

pub(crate) fn key_height(body_height: f32, landscape: bool) -> f32 {
    let (rows, spacing) = if landscape { (5., 48.) } else { (6., 52.) };
    ((body_height - spacing) / rows).floor().clamp(1., 72.)
}

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

    fn page_progress(&mut self, now: Instant) -> (f32, bool) {
        let target = f32::from(self.computer_second_page);
        match self.page_motion {
            PageMotion::Idle => (target, false),
            PageMotion::Dragging(offset) => (offset, false),
            PageMotion::Settling { from, started } => {
                let t = (now.saturating_duration_since(started).as_secs_f32() / 0.14).min(1.);
                if t >= 1. {
                    self.page_motion = PageMotion::Idle;
                }
                (from + (target - from) * (1. - (1. - t).powi(3)), t < 1.)
            }
        }
    }

    fn select_computer_page(&mut self, second: bool, cx: &mut Context<Self>) {
        let from = self.page_progress(Instant::now()).0;
        self.computer_second_page = second;
        self.page_motion = PageMotion::Settling {
            from,
            started: Instant::now(),
        };
        cx.notify();
    }

    pub(super) fn computer_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let landscape = window.viewport_size().width >= window.viewport_size().height;
        let height = key_height(self.body_height, landscape);
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
        let width = ((if self.body_width > 0. {
            self.body_width
        } else {
            window.viewport_size().width.into()
        }) - 8.)
            .max(1.);
        let (progress, animating) = self.page_progress(Instant::now());
        if animating {
            window.request_animation_frame();
        }
        let mut body = div().relative().size_full().overflow_hidden();
        for second in [false, true] {
            let rows = if second {
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
            let columns = if !landscape && second { 6 } else { 10 };
            let mut page = div()
                .absolute()
                .top_0()
                .left(px(width * (f32::from(second) - progress)))
                .w(px(width))
                .h_full()
                .flex()
                .flex_col()
                .gap_1();
            for row in rows {
                for keys in row.chunks(columns) {
                    let mut line = div().flex().gap_1().flex_shrink_0();
                    for (label, key) in keys {
                        line = line.child(self.computer_key(label, *key, height, cx));
                    }
                    for _ in keys.len()..columns {
                        line = line.child(div().flex_1().min_w_0());
                    }
                    page = page.child(line);
                }
            }
            body = body.child(page);
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
                        view.select_computer_page(second, cx);
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
                    .debug_selector(|| "computer-key-rows".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(
                        move |view, event: &gpui::ScrollWheelEvent, window, cx| {
                            let delta = event.delta.pixel_delta(window.line_height());
                            match event.touch_phase {
                                gpui::TouchPhase::Started | gpui::TouchPhase::Moved
                                    if delta.x.abs() > delta.y.abs()
                                        || matches!(view.page_motion, PageMotion::Dragging(_)) =>
                                {
                                    let from = view.page_progress(Instant::now()).0;
                                    view.page_motion = PageMotion::Dragging(
                                        (from - f32::from(delta.x) / width).clamp(0., 1.),
                                    );
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                                gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled
                                    if matches!(view.page_motion, PageMotion::Dragging(_)) =>
                                {
                                    let second = if event.touch_phase == gpui::TouchPhase::Cancelled
                                    {
                                        view.computer_second_page
                                    } else {
                                        view.page_progress(Instant::now()).0 >= 0.5
                                    };
                                    view.select_computer_page(second, cx);
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        },
                    ))
                    .child(body),
            )
            .child(pages.flex_shrink_0())
    }

    pub(super) fn shortcut_keyboard(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let landscape = window.viewport_size().width >= window.viewport_size().height;
        let height = key_height(self.body_height, landscape);
        let enabled = self.control.enabled.get();
        let supported = enabled && self.control.shortcuts.get();
        let mut grid = div().flex().flex_wrap().gap_1();
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
                    height,
                )
                .aria_label(format!("{label} {detail}"))
                .px_2()
                .gap_1()
                .child(label)
                .child(div().text_size(px(11.)).child(detail))
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
    id: impl Into<gpui::SharedString>,
    label: &str,
    enabled: bool,
    selected: bool,
    height: f32,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_keyboard::{KeyboardControl, KeyboardPage};
    use base::message_proto::key_event;
    use gpui_test_support::{TestAppContext, TouchEvent, TouchId, TouchPhase, point, size};
    use std::{cell::RefCell, rc::Rc};

    #[gpui_test_support::test]
    fn shortcut_keys_match_computer_key_height_and_keep_keyboard_extent(cx: &mut TestAppContext) {
        let (keyboard, cx) = cx.add_window_view(|window, cx| {
            let control = KeyboardControl::new(true);
            control.configure(true, true, true);
            let mut view = RemoteKeyboard::new(control, window, cx);
            view.page = KeyboardPage::Keys;
            view
        });
        for (width, height, body_height) in [(360., 800., 300.), (800., 360., 180.)] {
            cx.simulate_resize(size(px(width), px(height)));
            keyboard.update(cx, |view, cx| view.set_body_height(body_height, cx));
            keyboard.update(cx, |view, _| view.page = KeyboardPage::Keys);
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let key = cx.debug_bounds("computer-key-a").expect("letter key");
            let extent = cx.debug_bounds("remote-keyboard").unwrap();
            keyboard.update(cx, |view, _| view.page = KeyboardPage::Shortcuts);
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let shortcut = cx
                .debug_bounds("computer-shortcut-Ctrl+C")
                .expect("shortcut key");
            assert_eq!(shortcut.size.height, key.size.height);
            assert_eq!(cx.debug_bounds("remote-keyboard").unwrap(), extent);
            assert!(shortcut.size.width > key.size.width);
            keyboard.update(cx, |view, _| {
                view.page = KeyboardPage::Input;
                assert_eq!(view.input_notice(), None);
            });
        }
    }

    struct KeyboardViewport {
        keyboard: gpui::Entity<RemoteKeyboard>,
        height: f32,
    }
    impl gpui::Render for KeyboardViewport {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div()
                .w_full()
                .h(px(self.height))
                .child(self.keyboard.clone())
        }
    }

    #[gpui_test_support::test]
    fn both_computer_pages_fit_the_native_keyboard_height_without_vertical_scroll(
        cx: &mut TestAppContext,
    ) {
        let (stage, cx) = cx.add_window_view(|window, cx| {
            let keyboard = cx.new(|cx| {
                let control = KeyboardControl::new(true);
                control.configure(true, true, true);
                let mut view = RemoteKeyboard::new(control, window, cx);
                view.page = KeyboardPage::Keys;
                view
            });
            KeyboardViewport {
                keyboard,
                height: 250.,
            }
        });
        let keyboard = cx.update(|_, cx| stage.read(cx).keyboard.clone());
        for (width, screen_height, body_height) in [(360., 800., 250.), (800., 360., 160.)] {
            cx.simulate_resize(size(px(width), px(screen_height)));
            stage.update(cx, |view, _| view.height = body_height);
            for second in [false, true] {
                keyboard.update(cx, |view, cx| {
                    view.set_body_size(body_height, width, cx);
                    view.computer_second_page = second;
                    view.page_motion = PageMotion::Idle;
                });
                cx.update(|window, cx| window.draw(cx).clear(cx));
                let rows = cx.debug_bounds("computer-key-rows").unwrap();
                let last = cx
                    .debug_bounds(if second {
                        "computer-key-↓"
                    } else {
                        "computer-key-Enter"
                    })
                    .unwrap();
                assert!(last.origin.y >= rows.origin.y);
                assert!(
                    last.bottom() <= rows.bottom() + px(0.01),
                    "last key outside its page: {last:?}, page={rows:?}"
                );
                let before = last;
                for (phase, y) in [
                    (TouchPhase::Started, 80.),
                    (TouchPhase::Moved, 20.),
                    (TouchPhase::Ended, 20.),
                ] {
                    cx.simulate_event(TouchEvent {
                        id: TouchId(8),
                        phase,
                        position: point(px(150.), px(y)),
                        ..Default::default()
                    });
                }
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert_eq!(
                    cx.debug_bounds(if second {
                        "computer-key-↓"
                    } else {
                        "computer-key-Enter"
                    })
                    .unwrap(),
                    before,
                    "vertical gesture must not scroll the computer keyboard"
                );
            }
        }
    }

    #[gpui_test_support::test]
    fn horizontal_swipe_tracks_the_finger_before_snapping(cx: &mut TestAppContext) {
        let (keyboard, cx) = cx.add_window_view(|window, cx| {
            let control = KeyboardControl::new(true);
            control.configure(true, true, true);
            let mut view = RemoteKeyboard::new(control, window, cx);
            view.page = KeyboardPage::Keys;
            view.body_height = 276.;
            view
        });
        cx.simulate_resize(size(px(320.), px(640.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for (phase, x) in [(TouchPhase::Started, 240.), (TouchPhase::Moved, 140.)] {
            cx.simulate_event(TouchEvent {
                id: TouchId(1),
                phase,
                position: point(px(x), px(110.)),
                ..Default::default()
            });
        }
        cx.update(|window, cx| window.draw(cx).clear(cx));
        keyboard.update(cx, |view, _| {
            let offset = view.page_progress(Instant::now()).0;
            assert!(
                offset > 0.1 && offset < 0.9,
                "swipe snapped instead of tracking: {offset}"
            );
            assert!(!view.computer_second_page);
        });
        assert!(cx.debug_bounds("computer-key-1").unwrap().origin.x < px(0.));
    }

    #[gpui_test_support::test]
    fn horizontal_swipes_change_keys_without_typing(cx: &mut TestAppContext) {
        let (keyboard, cx) = cx.add_window_view(|window, cx| {
            let control = KeyboardControl::new(true);
            control.configure(true, true, true);
            let mut view = RemoteKeyboard::new(control, window, cx);
            view.page = KeyboardPage::Keys;
            view.body_height = 276.;
            view
        });
        cx.simulate_resize(size(px(320.), px(640.)));
        let sent = Rc::new(RefCell::new(Vec::new()));
        let output = sent.clone();
        let _subscription = cx.update(|window, cx| {
            window.subscribe(&keyboard, cx, move |_, event, _, _| {
                for command in &event.commands {
                    if let Command::Chord(event) = command {
                        let value = match event.union.as_ref().expect("key") {
                            key_event::Union::Chr(value) => *value as i32,
                            key_event::Union::ControlKey(value) => -value.value(),
                            _ => panic!("unexpected key"),
                        };
                        output.borrow_mut().push(value);
                    }
                }
            })
        });
        for (start, end, expected) in [
            (240., 60., -(ControlKey::F1 as i32)),
            (60., 240., '1' as i32),
        ] {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let before = sent.borrow().len();
            for (phase, x) in [
                (TouchPhase::Started, start),
                (TouchPhase::Moved, end),
                (TouchPhase::Ended, end),
            ] {
                if phase == TouchPhase::Ended {
                    cx.executor()
                        .advance_clock(std::time::Duration::from_millis(200));
                }
                cx.simulate_event(TouchEvent {
                    id: TouchId(1),
                    phase,
                    position: point(px(x), px(110.)),
                    ..TouchEvent::default()
                });
            }
            assert_eq!(sent.borrow().len(), before, "swiping typed a key");
            keyboard.update(cx, |view, _| {
                if let PageMotion::Settling { started, .. } = &mut view.page_motion {
                    *started -= Duration::from_millis(200);
                }
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_click(point(px(20.), px(110.)), gpui::Modifiers::default());
            assert_eq!(sent.borrow().last(), Some(&expected));
        }
    }
}
