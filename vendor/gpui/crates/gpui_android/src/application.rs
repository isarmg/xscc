use crate::{frame_clock::FrameClock, input::InputState, platform::State};
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
    let frame_clock = FrameClock::new(&state.app);
    let mut reported = Instant::now();
    let mut frames = 0u32;
    let mut frame_time = Duration::ZERO;
    let mut longest_frame = Duration::ZERO;
    let mut dispatch_time = Duration::ZERO;
    while !state.destroyed.get() {
        if state
            .window()
            .is_some_and(|w| w.can_draw() && w.dirty.get())
        {
            frame_clock.request();
        } else {
            frame_clock.cancel();
        }
        state.app.poll_events(None, |event| {
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
                    frame_clock.cancel();
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
                    frame_clock.cancel();
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
                    frame_clock.cancel();
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
        let dispatch_started = Instant::now();
        state.dispatcher.drain();
        dispatch_time += dispatch_started.elapsed();
        if let Some(window) = state.window() {
            window.refresh_insets();
            window.sync_ime();
            window.read_ime_events();
        }
        if !state.destroyed.get()
            && let Some(window) = state.window().filter(|w| w.can_draw() && w.dirty.get())
        {
            match frame_clock.take_ready() {
                Ok(true) => {
                    frame_clock.request();
                    let started = Instant::now();
                    window.request_frame(false);
                    let elapsed = started.elapsed();
                    frame_time += elapsed;
                    longest_frame = longest_frame.max(elapsed);
                    frames += 1;
                }
                Ok(false) => {}
                Err(error) => {
                    log::error!("Android frame clock failed: {error:#}");
                    state.finish();
                }
            }
        } else {
            frame_clock.cancel();
        }
        if let Some(window) = state.window() {
            window.sync_ime();
        }
        if reported.elapsed() >= Duration::from_secs(1) {
            if frames > 0 {
                log::info!(
                    "android_frame_pipeline callbacks={} elapsed_ms={:.1} frame_avg_ms={:.2} frame_max_ms={:.2} dispatch_ms={:.2}",
                    frames,
                    reported.elapsed().as_secs_f64() * 1000.,
                    frame_time.as_secs_f64() * 1000. / f64::from(frames),
                    longest_frame.as_secs_f64() * 1000.,
                    dispatch_time.as_secs_f64() * 1000.,
                );
            }
            reported = Instant::now();
            frames = 0;
            frame_time = Duration::ZERO;
            longest_frame = Duration::ZERO;
            dispatch_time = Duration::ZERO;
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
