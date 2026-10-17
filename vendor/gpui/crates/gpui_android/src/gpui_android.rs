//! Experimental Android single-window GPUI backend. Text input is not supported yet.
#![cfg(target_os = "android")]

mod application;
mod dispatcher;
mod fonts;
mod geometry;
mod input;
mod platform;
mod window;

pub use android_activity::AndroidApp;
pub use platform::AndroidPlatform;
use std::cell::{Cell, RefCell};

/// Construct on the `android_main` thread, with the Activity's own context.
pub fn application(app: AndroidApp) -> anyhow::Result<gpui::Application> {
    Ok(gpui::Application::with_platform(std::rc::Rc::new(
        AndroidPlatform::new(app)?,
    )))
}

pub(crate) struct Callback<T> {
    value: RefCell<Option<T>>,
    generation: Cell<u64>,
}

impl<T> Default for Callback<T> {
    fn default() -> Self {
        Self {
            value: RefCell::new(None),
            generation: Cell::new(0),
        }
    }
}

impl<T> Callback<T> {
    fn set(&self, value: T) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.value.replace(Some(value));
    }
    fn take(&self) -> Option<T> {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.value.borrow_mut().take()
    }
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut value = self.value.borrow_mut().take()?;
        let generation = self.generation.get();
        let result = f(&mut value);
        if self.generation.get() == generation {
            self.value.replace(Some(value));
        }
        Some(result)
    }
}
