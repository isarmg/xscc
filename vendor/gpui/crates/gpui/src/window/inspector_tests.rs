use super::*;
use crate::{
    DivInspectorState, InspectorElementId, MouseDownEvent, StyleRefinement, TestAppContext, div,
};

#[gpui::test]
fn inspector_only_tracks_its_open_window(cx: &mut TestAppContext) {
    let windows = [
        cx.add_window(|_, cx| InspectorTestRoot {
            child: cx.new(|_| InspectorTestView::default()),
        }),
        cx.add_window(|_, cx| InspectorTestRoot {
            child: cx.new(|_| InspectorTestView::default()),
        }),
    ];
    for window in windows {
        assert_closed_inspector(window.into(), cx);
    }
    for _ in 0..2 {
        cx.update_window(windows[0].into(), |root, window, cx| {
            let root = root.downcast::<InspectorTestRoot>().expect("test root");
            let child_widths = root.read(cx).child.read(cx).child_widths.clone();
            window.toggle_inspector(cx);
            window.draw(cx).clear(cx);
            assert_eq!(child_widths.borrow().as_slice(), &[px(10.); 3]);
            let path = window
                .rendered_frame
                .next_inspector_instance_ids
                .iter()
                .find_map(|(path, count)| (*count == 3).then(|| path.clone()))
                .expect("anonymous siblings share an inspector path");
            let selected_id = InspectorElementId {
                path,
                instance_id: 1,
            };
            let position = window
                .rendered_frame
                .hitboxes
                .iter()
                .find(|hitbox| {
                    window.rendered_frame.inspector_hitboxes.get(&hitbox.id) == Some(&selected_id)
                })
                .expect("middle sibling is pickable")
                .bounds
                .center();
            window.simulate_mouse_move(position, cx);
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
            assert!(!window.is_inspector_picking(cx));
            assert_eq!(
                window
                    .inspector
                    .as_ref()
                    .expect("open inspector")
                    .read(cx)
                    .active_element_id(),
                Some(&selected_id)
            );
            window.draw(cx).clear(cx);
            window.with_inspector_state::<DivInspectorState, _>(
                Some(&selected_id),
                cx,
                |state, _| {
                    let state = state.as_mut().expect("selected div has style state");
                    assert_eq!(
                        state.base_style.size.width,
                        Some(crate::Length::from(px(10.)))
                    );
                    state.base_style.size.width = Some(crate::Length::from(px(25.)));
                },
            );
            window.refresh();
            window.draw(cx).clear(cx);
            assert_eq!(
                child_widths.borrow().as_slice(),
                &[px(10.), px(25.), px(10.)]
            );
        })
        .expect("pick and edit a cached child");
        assert_closed_inspector(windows[1].into(), cx);
        windows[0]
            .update(cx, |_, window, cx| window.toggle_inspector(cx))
            .expect("close inspector");
        assert_closed_inspector(windows[0].into(), cx);
    }
}

struct InspectorTestRoot {
    child: Entity<InspectorTestView>,
}

impl Render for InspectorTestRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.child
            .clone()
            .cached(StyleRefinement::default().size(px(100.)))
    }
}

#[derive(Default)]
struct InspectorTestView {
    child_widths: Rc<RefCell<Vec<Pixels>>>,
}

impl Render for InspectorTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let child_widths = self.child_widths.clone();
        div()
            .on_children_prepainted(move |bounds, _, _| {
                *child_widths.borrow_mut() =
                    bounds.iter().map(|bounds| bounds.size.width).collect();
            })
            .with_dynamic_prepaint_order(|_, _| SmallVec::from_iter([2, 0, 1]))
            .id("inspector-root")
            .flex()
            .children((0..3).map(|_| div().size(px(10.)).flex_shrink_0()))
    }
}

fn assert_closed_inspector(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |root, window, cx| {
        window.draw(cx).clear(cx);
        window.draw(cx).clear(cx);
        let root = root.downcast::<InspectorTestRoot>().expect("test root");
        assert_eq!(
            root.read(cx)
                .child
                .read(cx)
                .child_widths
                .borrow()
                .as_slice(),
            &[px(10.); 3]
        );
        for frame in [&window.rendered_frame, &window.next_frame] {
            assert_eq!(frame.next_inspector_instance_ids.capacity(), 0);
            assert_eq!(frame.inspector_hitboxes.capacity(), 0);
        }
    })
    .expect("closed inspector has no bookkeeping and no style overrides");
}
