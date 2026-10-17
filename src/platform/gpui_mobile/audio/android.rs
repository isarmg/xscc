use super::{Failure, State};
use hbb_common::{anyhow::anyhow, bail, log, ResultType};
use jni::{
    objects::{GlobalRef, JClass, JObject, JString, JValue},
    sys::{jint, jlong},
    JNIEnv, JavaVM, NativeMethod,
};
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, LazyLock, Mutex, Weak,
    },
};

static NEXT_ID: AtomicI64 = AtomicI64::new(1);
static SESSIONS: LazyLock<Mutex<HashMap<i64, Weak<State>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Registration(i64);

impl Drop for Registration {
    fn drop(&mut self) {
        SESSIONS.lock().unwrap().remove(&self.0);
    }
}

pub(super) struct Session {
    vm: JavaVM,
    helper: GlobalRef,
    registration: Registration,
}

impl Session {
    pub(super) fn activate(state: Arc<State>) -> ResultType<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        SESSIONS.lock().unwrap().insert(id, Arc::downgrade(&state));
        let registration = Registration(id);

        // GameActivity owns both pointers and initializes the Application context.
        // Never reinitialize it or enter the unrelated Flutter/scrap JNI bridge.
        let context = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }?;
        let helper = {
            let mut env = vm.attach_current_thread()?;
            let result = create_helper(&mut env, context.context().cast(), id);
            result.map_err(|error| java_error(&mut env, error))?
        };
        let session = Self {
            vm,
            helper,
            registration,
        };
        {
            let mut env = session.vm.attach_current_thread()?;
            let result = env.call_method(session.helper.as_obj(), "activate", "()Z", &[]);
            let granted = result.map_err(|error| java_error(&mut env, error))?.z()?;
            if !granted {
                bail!("Android audio focus is unavailable. Keep the app in the foreground and turn sound on again.");
            }
        }
        Ok(session)
    }
}

fn create_helper(
    env: &mut JNIEnv<'_>,
    application: jni::sys::jobject,
    id: i64,
) -> jni::errors::Result<GlobalRef> {
    // JObject is a borrowed view, not an owned global reference.
    let application = unsafe { JObject::from_raw(application) };
    let loader = env
        .call_method(
            &application,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )?
        .l()?;
    let name = env.new_string("org.xscc.gpui.AndroidPlaybackAudio")?;
    // Native worker threads cannot find app classes through FindClass.
    let class = JClass::from(
        env.call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?,
    );
    env.register_native_methods(
        &class,
        &[NativeMethod {
            name: "nativeStopped".into(),
            sig: "(JI)V".into(),
            fn_ptr: native_stopped as *mut c_void,
        }],
    )?;
    let helper = env.new_object(
        &class,
        "(Landroid/content/Context;J)V",
        &[JValue::Object(&application), JValue::Long(id)],
    )?;
    env.new_global_ref(helper)
}

unsafe extern "system" fn native_stopped(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    id: jlong,
    reason: jint,
) {
    let state = SESSIONS.lock().unwrap().get(&id).and_then(Weak::upgrade);
    if let Some(state) = state {
        state.stop(if reason == 2 {
            Failure::RouteLost
        } else {
            Failure::FocusLost
        });
    }
}

fn java_error(env: &mut JNIEnv<'_>, error: jni::errors::Error) -> hbb_common::anyhow::Error {
    // Clear a pending exception before any teardown JNI call on this worker.
    if let Ok(exception) = env.exception_occurred() {
        if !exception.is_null() && env.exception_clear().is_ok() {
            let message = env
                .call_method(exception, "toString", "()Ljava/lang/String;", &[])
                .and_then(|value| value.l())
                .and_then(|value| {
                    env.get_string(&JString::from(value))
                        .map(|value| value.to_string_lossy().into_owned())
                });
            if let Ok(message) = message {
                return anyhow!("Android audio session: {message}");
            }
            if let Err(clear_error) = env.exception_clear() {
                log::warn!("Cannot clear Android audio exception: {clear_error}");
            }
        }
    }
    anyhow!("Android audio session: {error}")
}

impl Drop for Session {
    fn drop(&mut self) {
        // A queued Java callback can outlive close(), so it resolves a weak ID.
        SESSIONS.lock().unwrap().remove(&self.registration.0);
        let result = (|| -> ResultType<()> {
            let mut env = self.vm.attach_current_thread()?;
            let result = env.call_method(self.helper.as_obj(), "close", "()V", &[]);
            result.map_err(|error| java_error(&mut env, error))?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("Cannot release Android playback focus: {error}");
        }
    }
}
