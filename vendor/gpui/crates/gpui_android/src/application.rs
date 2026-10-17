use crate::{input::InputState, platform::State};
use android_activity::{
    InputStatus, MainEvent, PollEvent,
    input::{InputEvent, KeyAction, Keycode},
};
use gpui::AppLifecyclePhase;
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

pub(crate) fn run(state: &Rc<State>, launched: Box<dyn FnOnce()>) {
    let mut launched = Some(launched);
    let mut input = InputState::default();
    let mut next_frame = Instant::now();
    // A bounded fallback cadence. Android's FIFO swapchain handles presentation;
    // idle/background windows block indefinitely in the event loop, not a spin loop.
    const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
    while !state.destroyed.get() {
        let timeout = state
            .window()
            .filter(|w| w.can_draw() && w.dirty.get())
            .map(|_| {
                let remaining = next_frame.saturating_duration_since(Instant::now());
                // android-activity converts this duration to whole milliseconds.
                if remaining.is_zero() {
                    remaining
                } else {
                    remaining.max(Duration::from_millis(1))
                }
            });
        state.app.poll_events(timeout, |event| {
            let PollEvent::Main(event) = event else {
                return;
            };
            match event {
                MainEvent::InitWindow { .. } => {
                    if let Some(window) = state.window() {
                        if let Err(error) = window.attach_surface() {
                            log::error!("Android surface creation failed: {error:#}");
                            state.finish();
                        }
                    } else if let Some(launched) = launched.take() {
                        launched();
                        if let Some(window) = state.window() {
                            window.update_geometry();
                        }
                    }
                }
                MainEvent::TerminateWindow { .. } => {
                    cancel_touches(state, &mut input);
                    if let Some(window) = state.window() {
                        window.detach_surface();
                    }
                }
                MainEvent::WindowResized { .. }
                | MainEvent::ContentRectChanged { .. }
                | MainEvent::ConfigChanged { .. }
                | MainEvent::InsetsChanged { .. } => {
                    if let Some(window) = state.window() {
                        window.update_geometry();
                    }
                }
                MainEvent::RedrawNeeded { .. } => {
                    if let Some(window) = state.window() {
                        window.request_frame(true);
                    }
                }
                MainEvent::Start => {
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Foreground));
                }
                MainEvent::Resume { .. } => {
                    state.resumed.set(true);
                    if let Some(window) = state.window() {
                        window.set_visible(true);
                    }
                    state.lifecycle.with(|f| {
                        f(if state.focused.get() {
                            AppLifecyclePhase::Active
                        } else {
                            AppLifecyclePhase::Inactive
                        })
                    });
                }
                MainEvent::GainedFocus => {
                    state.focused.set(true);
                    if let Some(window) = state.window() {
                        window.set_active(true);
                    }
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Active));
                }
                MainEvent::LostFocus => {
                    cancel_touches(state, &mut input);
                    state.focused.set(false);
                    if let Some(window) = state.window() {
                        window.set_active(false);
                    }
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Inactive));
                }
                MainEvent::Pause => {
                    cancel_touches(state, &mut input);
                    state.resumed.set(false);
                    if let Some(window) = state.window() {
                        window.set_active(false);
                        window.set_visible(false);
                    }
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Inactive));
                }
                MainEvent::Stop => {
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Background));
                }
                MainEvent::LowMemory => {
                    state.memory.with(|f| f());
                }
                MainEvent::InputAvailable => {
                    read_input(state, &mut input);
                }
                MainEvent::Destroy => {
                    cancel_touches(state, &mut input);
                    if let Some(window) = state.window() {
                        window.set_visible(false);
                        window.sync_ime();
                        window.detach_surface();
                        if let Some(close) = window.close.take() {
                            close();
                        }
                    }
                    state.lifecycle.with(|f| f(AppLifecyclePhase::Disconnected));
                    state.quit.with(|f| f());
                    state.destroyed.set(true);
                }
                _ => {}
            }
            if let Some(window) = state.window() {
                window.sync_ime();
            }
        });
        state.dispatcher.drain();
        if let Some(window) = state.window() {
            window.refresh_insets();
            window.sync_ime();
            window.read_ime_events();
        }
        if !state.destroyed.get() && Instant::now() >= next_frame {
            if let Some(window) = state.window().filter(|w| w.can_draw() && w.dirty.get()) {
                window.request_frame(false);
            }
            next_frame = Instant::now() + FRAME_INTERVAL;
        }
        if let Some(window) = state.window() {
            window.sync_ime();
        }
    }
    state.dispatcher.shutdown();
}
fn cancel_touches(state: &State, input: &mut InputState) {
    let events = input.cancel_touches();
    if let Some(window) = state.window() {
        for event in events {
            window.input.with(|f| f(event));
        }
    }
}
fn read_input(state: &State, input: &mut InputState) {
    let Some(window) = state.window() else {
        return;
    };
    let mut events = match state.app.input_events_iter() {
        Ok(events) => events,
        Err(error) => {
            log::error!("Cannot acquire Android input events: {error}");
            return;
        }
    };
    while events.next(|event| {
        let mapped = match event {
            InputEvent::MotionEvent(event) => input.motion(event, window.scale.get()),
            InputEvent::KeyEvent(event) if event.key_code() == Keycode::Back => {
                if event.action() == KeyAction::Up {
                    if window.dismiss_keyboard() {
                        return InputStatus::Handled;
                    }
                    if window.back_enabled.get() {
                        window.back.with(|f| f());
                    } else if window.should_close.with(|f| f()).unwrap_or(true) {
                        state.finish();
                    }
                }
                return InputStatus::Handled;
            }
            InputEvent::KeyEvent(event) => input.key(event),
            // Untagged GameActivity snapshots/actions cannot identify a field
            // session. Only the generation-tagged Java bridge is authoritative.
            InputEvent::TextEvent(_) | InputEvent::TextAction(_) => return InputStatus::Handled,
            _ => return InputStatus::Unhandled,
        };
        if mapped.is_empty() {
            return InputStatus::Unhandled;
        }
        for event in mapped {
            window.input.with(|f| f(event));
        }
        InputStatus::Handled
    }) {}
}
