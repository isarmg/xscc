use gpui::{AndroidApp, Global};
use jni::{JavaVM, jni_sig, jni_str, objects::JObject};

pub struct DisplayLimit(pub Option<u32>);
impl Global for DisplayLimit {}

impl DisplayLimit {
    pub fn query(app: &AndroidApp) -> Self {
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
        let result = vm.attach_current_thread(|env| -> jni::errors::Result<f32> {
            let raw: jni::sys::jobject = app.activity_as_ptr().cast();
            let activity = unsafe { env.as_cast_raw::<JObject>(&raw)? };
            let result = env.call_method(
                &activity,
                jni_str!("remoteSessionMaximumRefreshRate"),
                jni_sig!("()F"),
                &[],
            );
            if result.is_err() && env.exception_check() {
                env.exception_clear();
            }
            result?.f()
        });
        match result {
            Ok(hz) if hz.is_finite() && hz >= 1.0 && hz <= 1000.0 => {
                log::info!("Android display streaming limit: {hz:.3} Hz");
                Self(Some(hz.round() as u32))
            }
            Ok(hz) => {
                log::warn!("Android display streaming limit is unavailable: {hz}");
                Self(None)
            }
            Err(error) => {
                log::warn!("Cannot read Android display streaming limit: {error}");
                Self(None)
            }
        }
    }
}
