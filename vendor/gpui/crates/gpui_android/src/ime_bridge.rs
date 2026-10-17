use android_activity::{
    AndroidApp, AndroidAppWaker,
    input::{TextInputState, TextSpan},
};
use jni::{
    Env, EnvUnowned, JValue, JavaVM,
    errors::{Result, ThrowRuntimeExAndDefault},
    jni_sig, jni_str,
    objects::{JClass, JObject, JString},
    sys::{jint, jlong},
};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};

pub(crate) struct Event {
    pub generation: u64,
    pub kind: EventKind,
}

pub(crate) enum EventKind {
    Text(TextInputState),
    Action(i32),
}

struct Registration {
    owner: u64,
    sender: Sender<Event>,
    waker: AndroidAppWaker,
}

static REGISTRATION: OnceLock<Mutex<Option<Registration>>> = OnceLock::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Bridge {
    app: AndroidApp,
    owner: u64,
    receiver: Receiver<Event>,
}

impl Bridge {
    pub(crate) fn new(app: &AndroidApp) -> Self {
        let (sender, receiver) = mpsc::channel();
        let owner = Self::next_generation();
        *REGISTRATION.get_or_init(Mutex::default).lock().unwrap() = Some(Registration {
            owner,
            sender,
            waker: app.create_waker(),
        });
        Self {
            app: app.clone(),
            owner,
            receiver,
        }
    }

    pub(crate) fn next_generation() -> u64 {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        // Java uses signed longs; never reuse a generation after exhaustion.
        assert!(generation > 0 && generation <= jlong::MAX as u64);
        generation
    }

    pub(crate) fn drain(&self) -> Vec<Event> {
        self.receiver.try_iter().collect()
    }

    pub(crate) fn configure(
        &self,
        generation: u64,
        input_type: i32,
        action: i32,
        options: i32,
        state: TextInputState,
        show: bool,
    ) {
        self.with_activity(move |env, activity| {
            let text = env.new_string(&state.text)?;
            let [
                selection_start,
                selection_end,
                composing_start,
                composing_end,
            ] = offsets(&state);
            env.call_method(
                activity,
                jni_str!("configureIme"),
                jni_sig!("(JIIILjava/lang/String;IIIIZ)V"),
                &[
                    JValue::Long(generation as jlong),
                    JValue::Int(input_type),
                    JValue::Int(action),
                    JValue::Int(options),
                    JValue::Object(&text),
                    JValue::Int(selection_start),
                    JValue::Int(selection_end),
                    JValue::Int(composing_start),
                    JValue::Int(composing_end),
                    JValue::Bool(show),
                ],
            )?;
            Ok(())
        });
    }

    pub(crate) fn hide(&self, generation: u64) {
        self.with_activity(move |env, activity| {
            env.call_method(
                activity,
                jni_str!("hideIme"),
                jni_sig!("(J)V"),
                &[JValue::Long(generation as jlong)],
            )?;
            Ok(())
        });
    }

    fn with_activity(
        &self,
        call: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<()> + Send + 'static,
    ) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| {
                let raw: jni::sys::jobject = app.activity_as_ptr().cast();
                // The Activity owns this global reference; AndroidApp keeps it alive.
                let activity = unsafe { env.as_cast_raw::<JObject>(&raw)? };
                let result = call(env, &activity);
                if result.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                result
            });
            if result.is_err() {
                // Do not include exception details that might contain entered text.
                log::trace!("Cannot update Android input connection");
            }
        }));
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(registration) = REGISTRATION.get() {
            let mut registration = registration.lock().unwrap();
            if registration
                .as_ref()
                .is_some_and(|entry| entry.owner == self.owner)
            {
                *registration = None;
            }
        }
    }
}

fn offsets(state: &TextInputState) -> [i32; 4] {
    let index = |offset: usize| offset.min(i32::MAX as usize) as i32;
    let (composing_start, composing_end) = state
        .compose_region
        .map_or((-1, -1), |span| (index(span.start), index(span.end)));
    [
        index(state.selection.start),
        index(state.selection.end),
        composing_start,
        composing_end,
    ]
}

fn send(event: Event) {
    if let Some(registration) = REGISTRATION.get() {
        let registration = registration.lock().unwrap();
        if let Some(registration) = registration.as_ref() {
            if registration.sender.send(event).is_ok() {
                registration.waker.wake();
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xscc_gpui_AndroidImeBridge_nativeTextChanged<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    generation: jlong,
    text: JString<'caller>,
    selection_start: jint,
    selection_end: jint,
    composing_start: jint,
    composing_end: jint,
) {
    env.with_env(|env| -> Result<()> {
        if generation <= 0 {
            return Ok(());
        }
        send(Event {
            generation: generation as u64,
            kind: EventKind::Text(TextInputState {
                text: text.try_to_string(env)?,
                selection: TextSpan {
                    start: selection_start.max(0) as usize,
                    end: selection_end.max(0) as usize,
                },
                compose_region: (composing_start >= 0 && composing_end >= 0).then_some(TextSpan {
                    start: composing_start.max(0) as usize,
                    end: composing_end.max(0) as usize,
                }),
            }),
        });
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xscc_gpui_AndroidImeBridge_nativeAction<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    generation: jlong,
    action: jint,
) {
    env.with_env(|_| -> Result<()> {
        if generation > 0 {
            send(Event {
                generation: generation as u64,
                kind: EventKind::Action(action),
            });
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
