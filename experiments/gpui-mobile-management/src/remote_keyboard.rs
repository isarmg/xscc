use crate::{
    keyboard_action::{Shortcut, editor_action, finish_editor_action},
    keyboard_input::{KeyboardState, Output},
    theme,
};
use base::message_proto::ControlKey;
use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, IntoElement, Pixels, Point, Render, Subscription, UTF16Selection, Window, canvas,
    div, prelude::*, rgb,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

pub enum Command {
    Text(String),
    Key(ControlKey),
    Shortcut(Shortcut),
}
pub struct KeyboardEvent {
    pub epoch: u64,
    pub commands: Vec<Command>,
}
pub struct KeyboardControl {
    state: RefCell<KeyboardState>,
    enabled: Cell<bool>,
    editing: Cell<bool>,
    unicode: Cell<bool>,
    shortcuts: Cell<bool>,
}
impl KeyboardControl {
    pub fn new(unicode: bool) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(KeyboardState::default()),
            enabled: Cell::new(false),
            editing: Cell::new(false),
            unicode: Cell::new(unicode),
            shortcuts: Cell::new(false),
        })
    }
    pub fn epoch(&self) -> u64 {
        self.state.borrow().epoch()
    }
    pub fn cancel(&self) {
        self.editing.set(false);
        self.state.borrow_mut().reset();
    }
    pub fn configure(&self, enabled: bool, unicode: bool, shortcuts: bool) {
        self.unicode.set(unicode);
        self.shortcuts.set(enabled && shortcuts);
        if self.enabled.replace(enabled) && !enabled {
            self.cancel();
        }
    }
}

pub struct RemoteKeyboard {
    pub control: Rc<KeyboardControl>,
    focus: FocusHandle,
    notice: String,
    _blur: Subscription,
}
impl EventEmitter<KeyboardEvent> for RemoteKeyboard {}
impl RemoteKeyboard {
    pub fn new(control: Rc<KeyboardControl>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |view: &mut Self, window, cx| {
            view.control.cancel();
            window.notify_text_input_changed(&view.focus);
            cx.notify();
        });
        Self {
            control,
            focus,
            notice: String::new(),
            _blur: blur,
        }
    }
    fn changed(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }
    fn publish(
        &mut self,
        result: Result<Vec<Output>, crate::keyboard_input::InputError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(outputs) => {
                self.notice.clear();
                let commands = outputs
                    .into_iter()
                    .map(|output| match output {
                        Output::Text(text) => Command::Text(text),
                        Output::Backspace => Command::Key(ControlKey::Backspace),
                        Output::Return => Command::Key(ControlKey::Return),
                        Output::Tab => Command::Key(ControlKey::Tab),
                    })
                    .collect::<Vec<_>>();
                if !commands.is_empty() {
                    cx.emit(KeyboardEvent {
                        epoch: self.control.epoch(),
                        commands,
                    });
                }
            }
            Err(_) => self.notice = "输入已取消：目标不支持该文字或编辑范围，请重新输入。".into(),
        }
        self.changed(window, cx);
    }
    fn shortcut(&mut self, shortcut: Shortcut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.control.enabled.get() || !self.control.shortcuts.get() {
            return;
        }
        shortcut.cancel_composition(&mut self.control.state.borrow_mut());
        cx.emit(KeyboardEvent {
            epoch: self.control.epoch(),
            commands: vec![Command::Shortcut(shortcut)],
        });
        self.changed(window, cx);
    }
    fn control_key(&mut self, key: ControlKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.control.enabled.get() {
            return;
        }
        self.control.state.borrow_mut().reset();
        cx.emit(KeyboardEvent {
            epoch: self.control.epoch(),
            commands: vec![Command::Key(key)],
        });
        self.changed(window, cx);
    }
}

fn byte_offset(text: &str, offset: usize) -> Option<usize> {
    let mut utf16 = 0;
    for (byte, ch) in text.char_indices() {
        if utf16 == offset {
            return Some(byte);
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == offset).then_some(text.len())
}
impl EntityInputHandler for RemoteKeyboard {
    fn accepts_text_input(&self, window: &mut Window, _: &mut Context<Self>) -> bool {
        self.control.enabled.get() && self.control.editing.get() && self.focus.is_focused(window)
    }
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let state = self.control.state.borrow();
        let text = state.document();
        let start = byte_offset(text, range.start)?;
        let end = byte_offset(text, range.end)?;
        let result = text.get(start..end)?.to_owned();
        *actual = Some(range);
        Some(result)
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.control.state.borrow().selection(),
            reversed: false,
        })
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text_input(window, cx) {
            return;
        }
        let epoch = self.control.epoch();
        if self
            .control
            .state
            .borrow_mut()
            .select(epoch, range)
            .is_err()
        {
            self.notice = "不能编辑未知远端范围，输入上下文已清空。".into();
        }
        self.changed(window, cx);
    }
    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(
            self.control
                .state
                .borrow()
                .document()
                .encode_utf16()
                .count(),
        )
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.control.state.borrow().marked()
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text_input(window, cx) {
            return;
        }
        let epoch = self.control.epoch();
        let result = self
            .control
            .state
            .borrow_mut()
            .mark(epoch, range, text, selected);
        self.publish(result, window, cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text_input(window, cx) {
            return;
        }
        let epoch = self.control.epoch();
        let result =
            self.control
                .state
                .borrow_mut()
                .commit(epoch, range, text, self.control.unicode.get());
        self.publish(result, window, cx);
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accepts_text_input(window, cx) {
            return;
        }
        let epoch = self.control.epoch();
        let result = self
            .control
            .state
            .borrow_mut()
            .unmark(epoch, self.control.unicode.get());
        self.publish(result, window, cx);
    }
    fn paste(&mut self, _: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = "此预览不支持剪贴板粘贴。".into();
        self.changed(window, cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.control.state.borrow().selection().end)
    }
}
impl Render for RemoteKeyboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(window.appearance());
        let entity = cx.entity();
        let focus = self.focus.clone();
        let enabled = self.control.enabled.get();
        div()
            .id("remote-keyboard")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, window, cx| {
                if !view.accepts_text_input(window, cx) {
                    return;
                }
                let Some(action) = editor_action(event) else {
                    return;
                };
                cx.stop_propagation();
                let result = finish_editor_action(
                    &mut view.control.state.borrow_mut(),
                    action,
                    view.control.unicode.get(),
                );
                view.publish(result, window, cx);
            }))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("open-remote-keyboard")
                    .min_h(gpui::px(44.))
                    .p_3()
                    .bg(rgb(palette.panel))
                    .child(if enabled {
                        "打开远程软键盘"
                    } else {
                        "远程键盘不可用"
                    })
                    .on_click(cx.listener(|view, _, window, cx| {
                        if !view.control.enabled.get() {
                            return;
                        }
                        view.control.state.borrow_mut().reset();
                        view.control.editing.set(true);
                        view.focus.focus(window, cx);
                        window.request_virtual_keyboard();
                        view.changed(window, cx);
                    })),
            )
            .child(
                div().child(
                    if self.notice.is_empty() {
                        if self.control.unicode.get() {
                            "文字直接提交 · 完整硬件键盘待接入"
                        } else {
                            "目标仅支持可打印 ASCII · 中文输入暂不可用"
                        }
                    } else {
                        &self.notice
                    }
                    .to_owned(),
                ),
            )
            .when(enabled, |view| {
                view.child(
                    div().flex().flex_wrap().gap_2().children(
                        [
                            ("退格", ControlKey::Backspace),
                            ("Delete", ControlKey::Delete),
                            ("Enter", ControlKey::Return),
                            ("Tab", ControlKey::Tab),
                            ("Esc", ControlKey::Escape),
                            ("←", ControlKey::LeftArrow),
                            ("→", ControlKey::RightArrow),
                            ("↑", ControlKey::UpArrow),
                            ("↓", ControlKey::DownArrow),
                        ]
                        .into_iter()
                        .map(|(label, key)| {
                            div()
                                .id(label)
                                .min_h(gpui::px(44.))
                                .p_2()
                                .bg(rgb(palette.panel))
                                .child(label)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.control_key(key, window, cx)
                                }))
                        }),
                    ),
                )
            })
            .when(enabled && self.control.shortcuts.get(), |view| {
                view.child(
                    div().flex().flex_wrap().gap_2().children(
                        [
                            ("全选 Ctrl+A", Shortcut::SelectAll),
                            ("撤销 Ctrl+Z", Shortcut::Undo),
                            ("保存 Ctrl+S", Shortcut::Save),
                        ]
                        .into_iter()
                        .map(|(label, shortcut)| {
                            div()
                                .id(label)
                                .min_h(gpui::px(44.))
                                .p_2()
                                .bg(rgb(palette.panel))
                                .child(label)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.shortcut(shortcut, window, cx)
                                }))
                        }),
                    ),
                )
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .w_full()
                .h(gpui::px(1.)),
            )
    }
}
