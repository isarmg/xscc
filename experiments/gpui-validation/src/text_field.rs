// Native input integration adapted from Zed's GPUI input example (Apache-2.0):
// https://github.com/zed-industries/zed/blob/28d0a3547f16b9609b301b48ab363bfca649c864/crates/gpui/examples/input.rs
// XSCC adds bounded single-line editing, protected secret fields and caret scrolling.
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, KeyBinding, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, canvas, div, fill, point, prelude::*, px, rgb,
    rgba, size,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const MAX_BYTES: usize = 4096;
const MASK: &str = "•";

actions!(
    xscc_text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Paste,
        Copy,
        Cut
    ]
);

pub fn init(cx: &mut App) {
    let context = Some("XsccTextField");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("shift-insert", Paste, context),
        KeyBinding::new("shift-delete", Cut, context),
        KeyBinding::new("ctrl-insert", Copy, context),
    ]);
}

pub struct TextField {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    secret: bool,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_line: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_x: Pixels,
    selecting: bool,
}

impl TextField {
    pub fn new(placeholder: &str, secret: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.to_owned().into(),
            secret,
            selection: 0..0,
            reversed: false,
            marked: None,
            last_line: None,
            last_bounds: None,
            scroll_x: px(0.),
            selecting: false,
        }
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.content.clear();
        self.selection = 0..0;
        self.reversed = false;
        self.marked = None;
        self.last_line = None;
        self.scroll_x = px(0.);
        self.selecting = false;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn move_to(&mut self, offset: usize, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let anchor = if extend {
            if self.reversed {
                self.selection.end
            } else {
                self.selection.start
            }
        } else {
            offset
        };
        self.selection = anchor.min(offset)..anchor.max(offset);
        self.reversed = offset < anchor;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn previous(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selection.is_empty() {
            self.previous(self.cursor())
        } else {
            self.selection.start
        };
        self.move_to(offset, false, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selection.is_empty() {
            self.next(self.cursor())
        } else {
            self.selection.end
        };
        self.move_to(offset, false, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous(self.cursor()), true, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next(self.cursor()), true, window, cx);
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, false, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), false, window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, true, window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), true, window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, false, window, cx);
        self.move_to(self.content.len(), true, window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.move_to(self.previous(self.cursor()), true, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.move_to(self.next(self.cursor()), true, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste_action(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selection.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selection.is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        window.request_virtual_keyboard();
        self.selecting = true;
        if event.click_count >= 2 {
            self.select_all(&SelectAll, window, cx);
        } else {
            self.move_to(
                self.index_at(event.position),
                event.modifiers.shift,
                window,
                cx,
            );
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.move_to(self.index_at(event.position), true, window, cx);
        }
    }

    fn index_at(&self, point: Point<Pixels>) -> usize {
        let (Some(line), Some(bounds)) = (&self.last_line, self.last_bounds) else {
            return 0;
        };
        let index = line.closest_index_for_x(point.x - bounds.left() + self.scroll_x);
        let offset = self.content_offset(index);
        self.content
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .chain(std::iter::once(self.content.len()))
            .take_while(|index| *index <= offset)
            .last()
            .unwrap_or(0)
    }

    fn display_offset(&self, offset: usize) -> usize {
        if self.secret {
            self.content[..offset].chars().count() * MASK.len()
        } else {
            offset
        }
    }

    fn content_offset(&self, offset: usize) -> usize {
        if self.secret {
            self.content
                .char_indices()
                .nth(offset / MASK.len())
                .map(|(index, _)| index)
                .unwrap_or(self.content.len())
        } else {
            floor_char_boundary(&self.content, offset)
        }
    }

    fn range_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        let start = utf16_to_utf8(&self.content, range.start.min(range.end));
        let end = utf16_to_utf8(&self.content, range.start.max(range.end));
        start..end
    }

    fn to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.content[..range.start].encode_utf16().count()
            ..self.content[..range.end].encode_utf16().count()
    }

    fn replace(&mut self, range: Option<Range<usize>>, text: &str) -> Range<usize> {
        let range = range
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        let available = MAX_BYTES.saturating_sub(self.content.len() - range.len());
        let insertion = single_line(text, available);
        self.content.replace_range(range.clone(), &insertion);
        let inserted = range.start..range.start + insertion.len();
        self.selection = inserted.end..inserted.end;
        self.reversed = false;
        self.last_line = None;
        inserted
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(range);
        *actual = Some(self.to_utf16(range.clone()));
        // Native selection services must not be able to copy the underlying secret either.
        Some(if self.secret {
            "*".repeat(self.content[range].encode_utf16().count())
        } else {
            self.content[range].to_owned()
        })
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selection.clone()),
            reversed: self.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = self.range_from_utf16(range);
        self.reversed = false;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|range| self.to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace(range, text);
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.replace(range, text);
        if let Some(selected) = selected {
            let text = &self.content[inserted.clone()];
            let start = utf16_to_utf8(text, selected.start.min(selected.end));
            let end = utf16_to_utf8(text, selected.start.max(selected.end));
            self.selection = inserted.start + start..inserted.start + end;
        }
        self.marked = (!inserted.is_empty()).then_some(inserted);
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_line.as_ref()?;
        let range = self.range_from_utf16(range);
        let x = bounds.left() - self.scroll_x;
        let start = x + line.x_for_index(self.display_offset(range.start));
        let end = x + line.x_for_index(self.display_offset(range.end));
        Some(Bounds::from_corners(
            point(start.min(end), bounds.top()),
            point(start.max(end), bounds.bottom()),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.last_bounds?.localize(&point)?;
        self.last_line.as_ref()?;
        Some(self.content[..self.index_at(point)].encode_utf16().count())
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .w_full()
            .min_w_0()
            .key_context("XsccTextField")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .bg(rgb(0x101923))
            .border_1()
            .border_color(if focused {
                rgb(0x60a5fa)
            } else {
                rgb(0x364352)
            })
            .rounded_md()
            .px_3()
            .py_2()
            .text_color(rgb(0xe5edf5))
            .text_size(px(14.))
            .line_height(px(22.))
            .overflow_hidden()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(text_canvas(cx.entity()))
    }
}

struct FieldPaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

fn text_canvas(field: Entity<TextField>) -> impl IntoElement {
    let prepaint_field = field.clone();
    canvas(
        move |bounds, window, cx| {
            let field = prepaint_field.read(cx);
            let style = window.text_style();
            let text: SharedString = if field.content.is_empty() {
                field.placeholder.clone()
            } else if field.secret {
                MASK.repeat(field.content.chars().count()).into()
            } else {
                field.content.clone().into()
            };
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: if field.content.is_empty() {
                    rgb(0x8b9aae).into()
                } else {
                    style.color
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = if let Some(marked) = &field.marked {
                let start = field.display_offset(marked.start);
                let end = field.display_offset(marked.end);
                vec![
                    TextRun {
                        len: start,
                        ..run.clone()
                    },
                    TextRun {
                        len: end - start,
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun {
                        len: text.len() - end,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            } else {
                vec![run]
            };
            let line = window.text_system().shape_line(
                text,
                style.font_size.to_pixels(window.rem_size()),
                &runs,
                None,
            );
            let cursor_x = line.x_for_index(field.display_offset(field.cursor()));
            let visible_width = (bounds.size.width - px(2.)).max(px(0.));
            let scroll_x = if field.content.is_empty() {
                px(0.)
            } else {
                field
                    .scroll_x
                    .min(cursor_x)
                    .max(cursor_x - visible_width)
                    .min((line.width() - visible_width).max(px(0.)))
                    .max(px(0.))
            };
            let origin_x = bounds.left() - scroll_x;
            let focused = field.focus.is_focused(window);
            let cursor = (focused && field.selection.is_empty()).then(|| {
                fill(
                    Bounds::new(
                        point(origin_x + cursor_x, bounds.top()),
                        size(px(2.), bounds.size.height),
                    ),
                    rgb(0x93c5fd),
                )
            });
            let selection = (focused && !field.selection.is_empty()).then(|| {
                let start =
                    origin_x + line.x_for_index(field.display_offset(field.selection.start));
                let end = origin_x + line.x_for_index(field.display_offset(field.selection.end));
                fill(
                    Bounds::from_corners(
                        point(start.min(end), bounds.top()),
                        point(start.max(end), bounds.bottom()),
                    ),
                    rgba(0x60a5fa55),
                )
            });
            FieldPaint {
                line,
                cursor,
                selection,
                scroll_x,
            }
        },
        move |bounds, mut paint, window, cx| {
            let focus = field.read(cx).focus.clone();
            window.handle_input(&focus, ElementInputHandler::new(bounds, field.clone()), cx);
            let line = paint.line;
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if let Some(selection) = paint.selection.take() {
                    window.paint_quad(selection);
                }
                if let Err(error) = line.paint(
                    point(bounds.left() - paint.scroll_x, bounds.top()),
                    window.line_height(),
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    eprintln!("Unable to paint text field: {error}");
                }
                if let Some(cursor) = paint.cursor.take() {
                    window.paint_quad(cursor);
                }
            });
            field.update(cx, |field, _| {
                field.last_line = Some(line);
                field.last_bounds = Some(bounds);
                field.scroll_x = paint.scroll_x;
            });
        },
    )
    .w_full()
    .h(px(22.))
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn utf16_to_utf8(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > offset {
            return index;
        }
    }
    text.len()
}

fn single_line(text: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut previous_cr = false;
    for ch in text.chars() {
        if ch == '\n' && previous_cr {
            previous_cr = false;
            continue;
        }
        previous_cr = ch == '\r';
        let ch = match ch {
            '\r' | '\n' | '\t' | '\u{85}' | '\u{2028}' | '\u{2029}' => ' ',
            ch if ch.is_control() => continue,
            ch => ch,
        };
        if result.len() + ch.len_utf8() > limit {
            break;
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_ranges_preserve_unicode_boundaries() {
        let text = "a😀e\u{301}中";
        assert_eq!(
            (0..=7)
                .map(|offset| utf16_to_utf8(text, offset))
                .collect::<Vec<_>>(),
            vec![0, 1, 1, 5, 6, 8, 11, 11]
        );
        for (index, _) in text.char_indices() {
            assert_eq!(
                utf16_to_utf8(text, text[..index].encode_utf16().count()),
                index
            );
        }
    }

    #[test]
    fn input_is_single_line_and_byte_bounded() {
        assert_eq!(single_line("a\r\nb\tc\u{2028}d\0", MAX_BYTES), "a b c d");
        assert_eq!(single_line("a😀b", 4), "a");
        assert_eq!(single_line("a😀b", 5), "a😀");
        assert_eq!(single_line(&"中".repeat(MAX_BYTES), MAX_BYTES).len(), 4095);
    }
}
