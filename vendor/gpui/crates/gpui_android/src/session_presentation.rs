use android_activity::AndroidApp;
use jni::{JValue, JavaVM, jni_sig, jni_str, objects::JObject};
use std::cell::Cell;

pub(crate) struct Presentation {
    app: AndroidApp,
    requested: Cell<bool>,
}

impl Presentation {
    pub(crate) fn new(app: &AndroidApp) -> Self {
        Self {
            app: app.clone(),
            requested: Cell::new(false),
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let previous = self.requested.replace(enabled);
        if !enabled && !previous {
            return;
        }
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let raw: jni::sys::jobject = app.activity_as_ptr().cast();
                // AndroidApp keeps the Activity's borrowed global reference alive.
                let activity = unsafe { env.as_cast_raw::<JObject>(&raw)? };
                let result = env.call_method(
                    &activity,
                    jni_str!("setRemoteSessionPresentation"),
                    jni_sig!("(Z)V"),
                    &[JValue::Bool(enabled)],
                );
                if result.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                result.map(|_| ())
            });
            if let Err(error) = result {
                log::warn!("Cannot request Android session presentation: {error}");
            }
        }));
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        self.set_enabled(false);
    }
}
