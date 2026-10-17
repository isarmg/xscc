use android_activity::{AndroidApp, AndroidAppWaker};
use anyhow::Context as _;
use gpui::{PlatformDispatcher, Priority, RunnableVariant};
use std::{
    collections::{BTreeMap, VecDeque},
    mem,
    sync::{Arc, Condvar, Mutex},
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Pending {
    ready: [VecDeque<RunnableVariant>; 3],
    timers: BTreeMap<Instant, Vec<RunnableVariant>>,
    stopped: bool,
}

impl Pending {
    fn push(&mut self, priority: Priority, runnable: RunnableVariant) {
        let index = match priority {
            Priority::High | Priority::RealtimeAudio => 0,
            Priority::Medium => 1,
            Priority::Low => 2,
        };
        self.ready[index].push_back(runnable);
    }

    fn pop(&mut self) -> Option<RunnableVariant> {
        self.ready.iter_mut().find_map(VecDeque::pop_front)
    }
}

#[derive(Default)]
struct Background {
    pending: Mutex<Pending>,
    wake: Condvar,
}

pub(crate) struct AndroidDispatcher {
    main_thread: ThreadId,
    main: Mutex<Pending>,
    waker: AndroidAppWaker,
    background: Arc<Background>,
}

impl AndroidDispatcher {
    pub(crate) fn new(app: &AndroidApp) -> anyhow::Result<Arc<Self>> {
        let background = Arc::new(Background::default());
        let worker_count = thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        for index in 0..worker_count {
            let worker = background.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("gpui-android-{index}"))
                .spawn(move || worker.run())
            {
                background.stop();
                return Err(error).context("failed to start Android background worker");
            }
        }
        Ok(Arc::new(Self {
            main_thread: thread::current().id(),
            main: Mutex::new(Pending::default()),
            waker: app.create_waker(),
            background,
        }))
    }

    pub(crate) fn drain(&self) {
        assert!(
            self.is_main_thread(),
            "foreground tasks require android_main"
        );
        // Yield to lifecycle and input processing even if a task keeps rescheduling itself.
        for _ in 0..256 {
            let runnable = self.main.lock().unwrap().pop();
            let Some(runnable) = runnable else {
                return;
            };
            runnable.run();
        }
        self.waker.wake();
    }

    pub(crate) fn shutdown(&self) {
        assert!(self.is_main_thread(), "shutdown requires android_main");
        let pending = {
            let mut main = self.main.lock().unwrap();
            main.stopped = true;
            mem::take(&mut main.ready)
        };
        // A foreground runnable may own a !Send future. Its destructor stays on this thread.
        drop(pending);
        self.background.stop();
    }
}

impl Drop for AndroidDispatcher {
    fn drop(&mut self) {
        self.background.stop();
        if !self.is_main_thread() {
            // Usually shutdown already emptied this queue. Never destroy a local future
            // on the worker that happens to release the final dispatcher reference.
            mem::forget(mem::take(&mut self.main.lock().unwrap().ready));
        }
    }
}

impl Background {
    fn run(&self) {
        loop {
            let runnable = {
                let mut pending = self.pending.lock().unwrap();
                loop {
                    if pending.stopped {
                        return;
                    }
                    let now = Instant::now();
                    while pending
                        .timers
                        .first_key_value()
                        .is_some_and(|(at, _)| *at <= now)
                    {
                        if let Some((_, timers)) = pending.timers.pop_first() {
                            pending.ready[0].extend(timers);
                        }
                    }
                    if let Some(runnable) = pending.pop() {
                        break runnable;
                    }
                    pending = if let Some((&deadline, _)) = pending.timers.first_key_value() {
                        self.wake
                            .wait_timeout(pending, deadline.saturating_duration_since(now))
                            .unwrap()
                            .0
                    } else {
                        self.wake.wait(pending).unwrap()
                    };
                }
            };
            runnable.run();
        }
    }

    fn stop(&self) {
        let abandoned = {
            let mut pending = self.pending.lock().unwrap();
            pending.stopped = true;
            (
                mem::take(&mut pending.ready),
                mem::take(&mut pending.timers),
            )
        };
        self.wake.notify_all();
        drop(abandoned);
    }
}

impl PlatformDispatcher for AndroidDispatcher {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        let mut pending = self.background.pending.lock().unwrap();
        if pending.stopped {
            drop(pending);
            drop(runnable);
            return;
        }
        pending.push(priority, runnable);
        self.background.wake.notify_one();
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, priority: Priority) {
        let mut main = self.main.lock().unwrap();
        if main.stopped {
            drop(main);
            if self.is_main_thread() {
                drop(runnable);
            } else {
                mem::forget(runnable);
            }
            return;
        }
        main.push(priority, runnable);
        drop(main);
        self.waker.wake();
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        let deadline = match Instant::now().checked_add(duration) {
            Some(deadline) => deadline,
            None => {
                log::error!("Android timer exceeds monotonic clock range; leaving task pending");
                mem::forget(runnable);
                return;
            }
        };
        let mut pending = self.background.pending.lock().unwrap();
        if pending.stopped {
            drop(pending);
            drop(runnable);
            return;
        }
        pending.timers.entry(deadline).or_default().push(runnable);
        self.background.wake.notify_all();
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        // A dedicated audio thread keeps long-lived audio loops out of the worker pool.
        // Android scheduling priority is left to the audio API that owns the stream.
        if let Err(error) = thread::Builder::new().name("gpui-audio".into()).spawn(f) {
            log::error!("failed to start Android audio thread: {error}");
        }
    }
}
