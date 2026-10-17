use android_activity::AndroidApp;
use jni::{
    Env, JValue, JavaVM,
    errors::Result,
    jni_sig, jni_str,
    objects::{JClass, JObject},
};
use std::{
    cell::Cell,
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub system: Insets,
    pub ime: Insets,
}

impl Snapshot {
    pub(crate) fn ime_visible(self) -> bool {
        self.ime.left > 0 || self.ime.top > 0 || self.ime.right > 0 || self.ime.bottom > 0
    }
}

pub(crate) struct InsetsTracker {
    sender: Sender<Snapshot>,
    receiver: Receiver<Snapshot>,
    latest: Cell<Option<Snapshot>>,
}

impl InsetsTracker {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            latest: Cell::new(None),
        }
    }

    pub(crate) fn request(&self, app: &AndroidApp) {
        let sender = self.sender.clone();
        let waker = app.create_waker();
        let activity_app = app.clone();
        app.run_on_java_main_thread(Box::new(move || {
            // AndroidApp keeps the VM and the Activity's borrowed global reference alive.
            let vm = unsafe { JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| {
                let result = query(env, &activity_app);
                if result.is_err() && env.exception_check() {
                    // This looper callback must not leave an exception pending in Java.
                    env.exception_clear();
                }
                result
            });
            match result {
                Ok(snapshot) => {
                    if sender.send(snapshot).is_ok() {
                        waker.wake();
                    }
                }
                Err(error) => log::trace!("Cannot read Android window insets: {error}"),
            }
        }));
    }

    pub(crate) fn latest(&self) -> Option<Snapshot> {
        for snapshot in self.receiver.try_iter() {
            self.latest.set(Some(snapshot));
        }
        self.latest.get()
    }
}

fn query(env: &mut Env<'_>, app: &AndroidApp) -> Result<Snapshot> {
    let raw: jni::sys::jobject = app.activity_as_ptr().cast();
    let activity = unsafe { env.as_cast_raw::<JObject>(&raw)? };
    // Looper callbacks have no application JNI frame, so resolve AndroidX through
    // the Activity's loader rather than relying on FindClass's default loader.
    let loader = env
        .call_method(
            &activity,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let type_name = env.new_string("androidx.core.view.WindowInsetsCompat$Type")?;
    let types = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&type_name)],
        )?
        .l()?;
    let types = env.as_cast::<JClass>(&types)?;
    let system_bars = env
        .call_static_method(&*types, jni_str!("systemBars"), jni_sig!("()I"), &[])?
        .i()?;
    let display_cutout = env
        .call_static_method(&*types, jni_str!("displayCutout"), jni_sig!("()I"), &[])?
        .i()?;
    let ime = env
        .call_static_method(&*types, jni_str!("ime"), jni_sig!("()I"), &[])?
        .i()?;
    Ok(Snapshot {
        system: query_type(env, &activity, system_bars | display_cutout)?,
        ime: query_type(env, &activity, ime)?,
    })
}

fn query_type(env: &mut Env<'_>, activity: &JObject<'_>, mask: i32) -> Result<Insets> {
    let insets = env
        .call_method(
            activity,
            jni_str!("getWindowInsets"),
            jni_sig!("(I)Landroidx/core/graphics/Insets;"),
            &[JValue::Int(mask)],
        )?
        .l()?;
    // GameActivity returns null both before root insets exist and for Insets.NONE.
    if insets.is_null() {
        return Ok(Insets::default());
    }
    Ok(Insets {
        left: env
            .get_field(&insets, jni_str!("left"), jni_sig!("I"))?
            .i()?,
        top: env
            .get_field(&insets, jni_str!("top"), jni_sig!("I"))?
            .i()?,
        right: env
            .get_field(&insets, jni_str!("right"), jni_sig!("I"))?
            .i()?,
        bottom: env
            .get_field(&insets, jni_str!("bottom"), jni_sig!("I"))?
            .i()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_retains_the_newest_reply() {
        let tracker = InsetsTracker::new();
        assert_eq!(tracker.latest(), None);
        let snapshot = Snapshot {
            ime: Insets {
                bottom: 300,
                ..Insets::default()
            },
            ..Snapshot::default()
        };
        tracker.sender.send(Snapshot::default()).unwrap();
        tracker.sender.send(snapshot).unwrap();
        assert_eq!(tracker.latest(), Some(snapshot));
        assert_eq!(tracker.latest(), Some(snapshot));
    }

    #[test]
    fn system_bars_do_not_imply_ime_visibility() {
        let mut snapshot = Snapshot {
            system: Insets {
                bottom: 30,
                ..Insets::default()
            },
            ..Snapshot::default()
        };
        assert!(!snapshot.ime_visible());
        snapshot.ime.bottom = 300;
        assert!(snapshot.ime_visible());
        snapshot.ime = Insets::default();
        assert!(!snapshot.ime_visible());
    }
}
