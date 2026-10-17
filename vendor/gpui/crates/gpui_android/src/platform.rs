use crate::{
    Callback,
    dispatcher::AndroidDispatcher,
    fonts,
    window::{AndroidDisplay, AndroidWindow, WindowState},
};
use android_activity::AndroidApp;
use anyhow::{anyhow, bail};
use futures::channel::oneshot;
use gpui::*;
use gpui_wgpu::GpuContext;
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
};

pub struct AndroidPlatform(pub(crate) Rc<State>);
pub(crate) struct State {
    pub app: AndroidApp,
    pub dispatcher: Arc<AndroidDispatcher>,
    pub background: BackgroundExecutor,
    pub foreground: ForegroundExecutor,
    pub text: Arc<dyn PlatformTextSystem>,
    pub gpu: GpuContext,
    pub window: RefCell<Weak<WindowState>>,
    pub display: Rc<AndroidDisplay>,
    pub resumed: Cell<bool>,
    pub focused: Cell<bool>,
    pub destroyed: Cell<bool>,
    pub lifecycle: Callback<Box<dyn FnMut(AppLifecyclePhase)>>,
    pub memory: Callback<Box<dyn FnMut()>>,
    pub quit: Callback<Box<dyn FnMut() -> bool>>,
    pub displays_changed: Callback<Box<dyn FnMut()>>,
}
impl AndroidPlatform {
    pub fn new(app: AndroidApp) -> anyhow::Result<Self> {
        let dispatcher = AndroidDispatcher::new(&app)?;
        app.enable_motion_axis(android_activity::input::Axis::Vscroll);
        app.enable_motion_axis(android_activity::input::Axis::Hscroll);
        Ok(Self(Rc::new(State {
            background: BackgroundExecutor::new(dispatcher.clone()),
            foreground: ForegroundExecutor::new(dispatcher.clone()),
            dispatcher,
            app,
            text: fonts::text_system()?,
            gpu: Rc::new(RefCell::new(None)),
            window: RefCell::new(Weak::new()),
            display: Rc::new(AndroidDisplay::default()),
            resumed: Cell::new(false),
            focused: Cell::new(false),
            destroyed: Cell::new(false),
            lifecycle: Callback::default(),
            memory: Callback::default(),
            quit: Callback::default(),
            displays_changed: Callback::default(),
        })))
    }
}
impl State {
    pub fn window(&self) -> Option<Rc<WindowState>> {
        self.window.borrow().upgrade()
    }
    pub fn finish(&self) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            // The Activity owns this borrowed global reference. Never wrap it as
            // an owned JNI local/global; the AndroidApp keeps it alive here.
            let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let raw: jni::sys::jobject = app.activity_as_ptr().cast();
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw)? };
                env.call_method(activity, jni::jni_str!("finish"), jni::jni_sig!("()V"), &[])?;
                Ok(())
            });
            if let Err(error) = result {
                log::error!("Cannot finish Android Activity: {error}");
            }
        }));
    }
}
fn unsupported<T>() -> anyhow::Result<T> {
    Err(anyhow!(
        "This Android GPUI rendering slice does not support this operation"
    ))
}
fn unsupported_receiver<T>() -> oneshot::Receiver<anyhow::Result<T>> {
    let (sender, receiver) = oneshot::channel();
    if sender.send(unsupported()).is_err() {
        log::debug!("Android operation receiver closed");
    }
    receiver
}
struct KeyboardLayout;
impl PlatformKeyboardLayout for KeyboardLayout {
    fn id(&self) -> &str {
        "android-physical"
    }
    fn name(&self) -> &str {
        "Android physical keyboard"
    }
}
impl Platform for AndroidPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.0.background.clone()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.foreground.clone()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.0.text.clone()
    }
    fn run(&self, launched: Box<dyn FnOnce()>) {
        crate::application::run(&self.0, launched);
    }
    fn quit(&self) {
        if self.0.quit.with(|f| f()).unwrap_or(true) {
            self.0.finish();
        }
    }
    fn restart(&self, _: Option<PathBuf>, _: Vec<OsString>) {
        log::warn!("Android Activity restart is unsupported");
    }
    fn activate(&self, _: bool) {}
    fn hide(&self) {
        log::warn!("Android Activity hiding is unsupported");
    }
    fn hide_other_apps(&self) {}
    fn unhide_other_apps(&self) {}
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![self.0.display.clone()]
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.0.display.clone())
    }
    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.displays_changed.set(callback);
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.0.window().filter(|w| w.active.get()).map(|w| w.handle)
    }
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        _: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        if self.0.window().is_some() {
            bail!("Android GPUI supports one Activity window");
        }
        let window = AndroidWindow::new(handle, &self.0)?;
        self.0.window.replace(Rc::downgrade(&window.0));
        Ok(Box::new(window))
    }
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }
    fn open_url(&self, _: &str) {
        log::warn!("Android URL opening is unsupported in the rendering slice");
    }
    fn on_open_urls(&self, _: Box<dyn FnMut(Vec<String>)>) {}
    fn register_url_scheme(&self, _: &str) -> Task<anyhow::Result<()>> {
        Task::ready(unsupported())
    }
    fn prompt_for_paths(
        &self,
        _: PathPromptOptions,
    ) -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
        unsupported_receiver()
    }
    fn prompt_for_new_path(
        &self,
        _: &Path,
        _: Option<&str>,
    ) -> oneshot::Receiver<anyhow::Result<Option<PathBuf>>> {
        unsupported_receiver()
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }
    fn reveal_path(&self, _: &Path) {
        log::warn!("Android file reveal is unsupported");
    }
    fn open_with_system(&self, _: &Path) {
        log::warn!("Android file opening is unsupported");
    }
    fn on_quit(&self, f: Box<dyn FnMut() -> bool>) {
        self.0.quit.set(f);
    }
    fn on_reopen(&self, _: Box<dyn FnMut()>) {}
    fn on_system_sleep(&self, _: Box<dyn FnMut()>) {}
    fn on_system_wake(&self, _: Box<dyn FnMut()>) {}
    fn on_app_lifecycle(&self, f: Box<dyn FnMut(AppLifecyclePhase)>) {
        self.0.lifecycle.set(f);
    }
    fn on_memory_warning(&self, f: Box<dyn FnMut()>) {
        self.0.memory.set(f);
    }
    fn set_menus(&self, _: Vec<Menu>, _: &Keymap) {}
    fn set_dock_menu(&self, _: Vec<MenuItem>, _: &Keymap) {}
    fn on_app_menu_action(&self, _: Box<dyn FnMut(&dyn Action)>) {}
    fn on_will_open_app_menu(&self, _: Box<dyn FnMut()>) {}
    fn on_validate_app_menu_command(&self, _: Box<dyn FnMut(&dyn Action) -> bool>) {}
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
    fn on_thermal_state_change(&self, _: Box<dyn FnMut()>) {}
    fn prevent_idle_sleep(&self, _: &str) -> Task<anyhow::Result<ActivityGuard>> {
        Task::ready(unsupported())
    }
    fn app_path(&self) -> anyhow::Result<PathBuf> {
        self.0
            .app
            .internal_data_path()
            .ok_or_else(|| anyhow!("Android internal data directory is unavailable"))
    }
    fn path_for_auxiliary_executable(&self, _: &str) -> anyhow::Result<PathBuf> {
        unsupported()
    }
    fn set_cursor_style(&self, _: CursorStyle) {}
    fn hide_cursor_until_mouse_moves(&self) {}
    fn is_cursor_visible(&self) -> bool {
        false
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        log::warn!("Android clipboard is not supported in the rendering slice");
        None
    }
    fn write_to_clipboard(&self, _: ClipboardItem) {
        log::warn!("Android clipboard is not supported in the rendering slice");
    }
    fn write_credentials(&self, _: &str, _: &str, _: &[u8]) -> Task<anyhow::Result<()>> {
        Task::ready(unsupported())
    }
    fn read_credentials(&self, _: &str) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>> {
        Task::ready(unsupported())
    }
    fn delete_credentials(&self, _: &str) -> Task<anyhow::Result<()>> {
        Task::ready(unsupported())
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(KeyboardLayout)
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
    fn on_keyboard_layout_change(&self, _: Box<dyn FnMut()>) {}
}
