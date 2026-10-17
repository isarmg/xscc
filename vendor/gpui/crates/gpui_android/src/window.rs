use crate::{Callback, geometry::density, platform::State};
use anyhow::{anyhow, bail};
use futures::channel::oneshot;
use gpui::*;
use gpui_wgpu::{WgpuRenderer, WgpuSurfaceConfig};
use ndk::native_window::NativeWindow;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle as RawWindowHandle,
};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Debug)]
struct SurfaceWindow(NativeWindow);
impl HasWindowHandle for SurfaceWindow {
    fn window_handle(&self) -> Result<RawWindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}
impl HasDisplayHandle for SurfaceWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::android())
    }
}
#[derive(Debug, Default)]
pub(crate) struct AndroidDisplay {
    pub bounds: Cell<Bounds<Pixels>>,
}
impl PlatformDisplay for AndroidDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(0)
    }
    fn uuid(&self) -> anyhow::Result<uuid::Uuid> {
        Ok(uuid::Uuid::from_bytes(*b"android-gpui\0\0\0\x01"))
    }
    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }
    fn refresh_interval(&self) -> Option<Duration> {
        None
    }
}

pub(crate) struct AndroidWindow(pub Rc<WindowState>);
pub(crate) struct WindowState {
    pub handle: AnyWindowHandle,
    pub platform: Weak<State>,
    native: RefCell<Option<SurfaceWindow>>,
    renderer: RefCell<WgpuRenderer>,
    pub active: Cell<bool>,
    pub visible: Cell<bool>,
    pub dirty: Cell<bool>,
    size: Cell<Size<Pixels>>,
    pub scale: Cell<f32>,
    viewport: Cell<Bounds<Pixels>>,
    insets_tracker: crate::insets::InsetsTracker,
    pub(crate) native_insets: Cell<Option<crate::insets::Snapshot>>,
    pub(crate) input_handler: Callback<PlatformInputHandler>,
    pub(crate) ime: RefCell<crate::ime::ImeState>,
    pub(crate) ime_bridge: crate::ime_bridge::Bridge,
    pub(crate) ime_configuration: RefCell<TextInputConfiguration>,
    pub(crate) ime_show: Cell<bool>,
    request_frame: Callback<Box<dyn FnMut(RequestFrameOptions)>>,
    pub input: Callback<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_changed: Callback<Box<dyn FnMut(bool)>>,
    visibility_changed: Callback<Box<dyn FnMut(WindowVisibility)>>,
    resized: Callback<Box<dyn FnMut(Size<Pixels>, f32)>>,
    viewport_changed: Callback<Box<dyn FnMut()>>,
    insets_changed: Callback<Box<dyn FnMut(WindowInsets)>>,
    pub should_close: Callback<Box<dyn FnMut() -> bool>>,
    pub close: Callback<Box<dyn FnOnce()>>,
    pub back: Callback<Box<dyn FnMut()>>,
    pub back_enabled: Cell<bool>,
}
impl AndroidWindow {
    pub fn new(handle: AnyWindowHandle, platform: &Rc<State>) -> anyhow::Result<Self> {
        let native = SurfaceWindow(
            platform
                .app
                .native_window()
                .ok_or_else(|| anyhow!("Open the GPUI window after Android InitWindow"))?,
        );
        let scale = density(platform.app.config().density());
        let size = physical_size(&native)?;
        let renderer = WgpuRenderer::new(
            platform.gpu.clone(),
            &native,
            WgpuSurfaceConfig {
                size,
                transparent: false,
                preferred_present_mode: None,
            },
            None,
        )?;
        let logical = gpui::size(
            px(size.width.0 as f32 / scale),
            px(size.height.0 as f32 / scale),
        );
        platform
            .display
            .bounds
            .set(Bounds::new(Point::default(), logical));
        Ok(Self(Rc::new(WindowState {
            handle,
            platform: Rc::downgrade(platform),
            native: RefCell::new(Some(native)),
            renderer: RefCell::new(renderer),
            active: Cell::new(platform.focused.get()),
            visible: Cell::new(platform.resumed.get()),
            dirty: Cell::new(true),
            size: Cell::new(logical),
            scale: Cell::new(scale),
            viewport: Cell::new(Bounds::new(Point::default(), logical)),
            insets_tracker: crate::insets::InsetsTracker::new(),
            native_insets: Cell::new(None),
            input_handler: Callback::default(),
            ime: RefCell::default(),
            ime_bridge: crate::ime_bridge::Bridge::new(&platform.app),
            ime_configuration: RefCell::default(),
            ime_show: Cell::new(false),
            request_frame: Callback::default(),
            input: Callback::default(),
            active_changed: Callback::default(),
            visibility_changed: Callback::default(),
            resized: Callback::default(),
            viewport_changed: Callback::default(),
            insets_changed: Callback::default(),
            should_close: Callback::default(),
            close: Callback::default(),
            back: Callback::default(),
            back_enabled: Cell::new(false),
        })))
    }
}
fn physical_size(window: &SurfaceWindow) -> anyhow::Result<Size<DevicePixels>> {
    let (width, height) = (window.0.width(), window.0.height());
    if width <= 0 || height <= 0 {
        bail!("Android native window has zero area");
    }
    Ok(size(DevicePixels(width), DevicePixels(height)))
}
impl WindowState {
    pub fn can_draw(&self) -> bool {
        self.visible.get() && self.native.borrow().is_some()
    }
    pub fn request_frame(&self, force: bool) {
        if !self.visible.get() || self.native.borrow().is_none() {
            return;
        }
        self.dirty.set(false);
        self.request_frame.with(|f| {
            f(RequestFrameOptions {
                require_presentation: true,
                force_render: force,
                ..Default::default()
            })
        });
    }
    pub fn schedule_frame(&self) {
        self.dirty.set(true);
        if let Some(platform) = self.platform.upgrade() {
            platform.app.create_waker().wake();
        }
    }
    pub fn set_active(&self, active: bool) {
        if self.active.replace(active) != active {
            self.active_changed.with(|f| f(active));
        }
    }
    pub fn set_visible(&self, visible: bool) {
        if self.visible.replace(visible) != visible {
            self.visibility_changed.with(|f| {
                f(if visible {
                    WindowVisibility::Visible
                } else {
                    WindowVisibility::Hidden
                })
            });
        }
        if visible {
            self.schedule_frame();
        }
    }
    pub fn detach_surface(&self) {
        self.renderer.borrow_mut().unconfigure_surface();
        self.native.borrow_mut().take();
        self.dirty.set(false);
    }
    pub fn attach_surface(&self) -> anyhow::Result<()> {
        let platform = self
            .platform
            .upgrade()
            .ok_or_else(|| anyhow!("Android platform was dropped"))?;
        let native = SurfaceWindow(
            platform
                .app
                .native_window()
                .ok_or_else(|| anyhow!("Android InitWindow has no native window"))?,
        );
        let size = physical_size(&native)?;
        let instance = platform
            .gpu
            .borrow()
            .as_ref()
            .ok_or_else(|| anyhow!("GPU context missing"))?
            .instance
            .clone();
        self.renderer.borrow_mut().replace_surface(
            &native,
            WgpuSurfaceConfig {
                size,
                transparent: false,
                preferred_present_mode: None,
            },
            &instance,
        )?;
        self.native.replace(Some(native));
        self.update_geometry();
        self.schedule_frame();
        Ok(())
    }
    pub(crate) fn refresh_insets(&self) {
        if let Some(snapshot) = self.insets_tracker.latest() {
            if self.native_insets.replace(Some(snapshot)) != Some(snapshot) {
                self.ime_visibility_changed(snapshot.ime_visible());
                self.update_geometry_inner(false);
                self.insets_changed.with(|f| f(self.insets()));
            }
        }
    }
    pub fn update_geometry(&self) {
        self.update_geometry_inner(true);
    }
    fn update_geometry_inner(&self, request_insets: bool) {
        let Some(platform) = self.platform.upgrade() else {
            return;
        };
        if request_insets {
            self.insets_tracker.request(&platform.app);
        }
        let scale = density(platform.app.config().density());
        let physical = self
            .native
            .borrow()
            .as_ref()
            .and_then(|native| physical_size(native).ok());
        let Some(physical) = physical else {
            return;
        };
        let logical = size(
            px(physical.width.0 as f32 / scale),
            px(physical.height.0 as f32 / scale),
        );
        let changed = self.size.replace(logical) != logical || self.scale.get() != scale;
        self.scale.set(scale);
        self.renderer.borrow_mut().update_drawable_size(physical);
        platform
            .display
            .bounds
            .set(Bounds::new(Point::default(), logical));
        let rect = platform.app.content_rect();
        let left = (rect.left as f32 / scale).clamp(0., f32::from(logical.width));
        let top = (rect.top as f32 / scale).clamp(0., f32::from(logical.height));
        let right = (rect.right as f32 / scale).clamp(left, f32::from(logical.width));
        let bottom = (rect.bottom as f32 / scale).clamp(top, f32::from(logical.height));
        let viewport = if right > left && bottom > top {
            Bounds::new(
                point(px(left), px(top)),
                size(px(right - left), px(bottom - top)),
            )
        } else {
            Bounds::new(Point::default(), logical)
        };
        let viewport = if self.native_insets.get().is_some() {
            let edges = self.insets().effective();
            let left = edges.left.min(logical.width);
            let top = edges.top.min(logical.height);
            Bounds::new(
                point(left, top),
                size(
                    (logical.width - left - edges.right).max(px(0.)),
                    (logical.height - top - edges.bottom).max(px(0.)),
                ),
            )
        } else {
            viewport
        };
        let viewport_changed = self.viewport.replace(viewport) != viewport;
        if changed {
            self.resized.with(|f| f(logical, scale));
            platform.displays_changed.with(|f| f());
        }
        if viewport_changed {
            self.viewport_changed.with(|f| f());
            self.insets_changed.with(|f| f(self.insets()));
        }
        self.schedule_frame();
    }
    fn insets(&self) -> WindowInsets {
        if let Some(snapshot) = self.native_insets.get() {
            let scale = self.scale.get();
            let edges = |value: crate::insets::Insets| Edges {
                top: px(value.top.max(0) as f32 / scale),
                left: px(value.left.max(0) as f32 / scale),
                right: px(value.right.max(0) as f32 / scale),
                bottom: px(value.bottom.max(0) as f32 / scale),
            };
            return WindowInsets {
                safe_area: edges(snapshot.system),
                ime: edges(snapshot.ime),
            };
        }
        let viewport = self.viewport.get();
        let full = self.size.get();
        WindowInsets {
            safe_area: Edges {
                top: viewport.origin.y,
                left: viewport.origin.x,
                right: (full.width - viewport.right()).max(px(0.)),
                bottom: (full.height - viewport.bottom()).max(px(0.)),
            },
            ..Default::default()
        }
    }
}
impl Drop for WindowState {
    fn drop(&mut self) {
        self.ime_bridge
            .hide(crate::ime_bridge::Bridge::next_generation());
        self.renderer.get_mut().destroy();
        self.native.get_mut().take();
    }
}
impl HasWindowHandle for AndroidWindow {
    fn window_handle(&self) -> Result<RawWindowHandle<'_>, HandleError> {
        // Activity surfaces can disappear through another Rc during lifecycle
        // callbacks. Only the renderer's owned SurfaceWindow may expose a handle.
        Err(HandleError::Unavailable)
    }
}
impl HasDisplayHandle for AndroidWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::android())
    }
}
impl PlatformWindow for AndroidWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), self.0.size.get())
    }
    fn is_maximized(&self) -> bool {
        true
    }
    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Fullscreen(self.bounds())
    }
    fn content_size(&self) -> Size<Pixels> {
        self.0.size.get()
    }
    fn visual_viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.viewport.get()
    }
    fn on_visual_viewport_changed(&self, f: Box<dyn FnMut()>) {
        self.0.viewport_changed.set(f);
    }
    fn resize(&mut self, _: Size<Pixels>) {}
    fn scale_factor(&self) -> f32 {
        self.0.scale.get()
    }
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.0
            .platform
            .upgrade()
            .map(|p| p.display.clone() as Rc<dyn PlatformDisplay>)
    }
    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }
    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }
    fn capslock(&self) -> Capslock {
        Capslock::default()
    }
    fn set_input_handler(&mut self, handler: PlatformInputHandler) {
        self.0.input_handler.set(handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.input_handler.take()
    }
    fn prompt(
        &self,
        _: PromptLevel,
        _: &str,
        _: Option<&str>,
        _: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        None
    }
    fn activate(&self) {}
    fn is_active(&self) -> bool {
        self.0.active.get()
    }
    fn visibility(&self) -> WindowVisibility {
        if self.0.visible.get() {
            WindowVisibility::Visible
        } else {
            WindowVisibility::Hidden
        }
    }
    fn is_hovered(&self) -> bool {
        false
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }
    fn set_title(&mut self, _: &str) {}
    fn set_background_appearance(&self, _: WindowBackgroundAppearance) {}
    fn minimize(&self) {}
    fn zoom(&self) {}
    fn toggle_fullscreen(&self) {}
    fn is_fullscreen(&self) -> bool {
        true
    }
    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        let weak = Rc::downgrade(&self.0);
        Some(Rc::new(move || {
            if let Some(window) = weak.upgrade() {
                window.schedule_frame();
            }
        }))
    }
    fn on_request_frame(&self, f: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.request_frame.set(f);
    }
    fn on_input(&self, f: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.input.set(f);
    }
    fn on_active_status_change(&self, f: Box<dyn FnMut(bool)>) {
        self.0.active_changed.set(f);
    }
    fn on_visibility_change(&self, f: Box<dyn FnMut(WindowVisibility)>) {
        self.0.visibility_changed.set(f);
    }
    fn on_hover_status_change(&self, _: Box<dyn FnMut(bool)>) {}
    fn on_resize(&self, f: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.resized.set(f);
    }
    fn on_moved(&self, _: Box<dyn FnMut()>) {}
    fn on_display_changed(&self, _: Box<dyn FnMut()>) {}
    fn on_should_close(&self, f: Box<dyn FnMut() -> bool>) {
        self.0.should_close.set(f);
    }
    fn on_hit_test_window_control(&self, _: Box<dyn FnMut() -> Option<WindowControlArea>>) {}
    fn on_close(&self, f: Box<dyn FnOnce()>) {
        self.0.close.set(f);
    }
    fn on_appearance_changed(&self, _: Box<dyn FnMut()>) {}
    fn draw(&self, scene: &Scene) {
        if !self.0.visible.get() || self.0.native.borrow().is_none() {
            return;
        }
        let mut renderer = self.0.renderer.borrow_mut();
        if renderer.device_lost() {
            if let Some(native) = self.0.native.borrow().as_ref() {
                if let Err(error) = renderer.recover(native) {
                    log::error!("Android GPU recovery failed: {error:#}");
                    if let Some(platform) = self.0.platform.upgrade() {
                        platform.finish();
                    }
                    return;
                }
            }
        }
        let presented = renderer.draw(scene);
        if !presented || renderer.needs_redraw() {
            self.0.schedule_frame();
        }
    }
    fn schedule_frame(&self) {
        self.0.schedule_frame();
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.renderer.borrow().sprite_atlas().clone()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.0.renderer.borrow().gpu_specs()
    }
    fn update_ime_position(&self, _: Bounds<Pixels>) {}
    fn insets(&self) -> WindowInsets {
        self.0.insets()
    }
    fn on_insets_changed(&self, f: Box<dyn FnMut(WindowInsets)>) {
        self.0.insets_changed.set(f);
    }
    fn set_back_handler(&self, f: Box<dyn FnMut()>) {
        self.0.back.set(f);
    }
    fn set_back_enabled(&self, enabled: bool) {
        self.0.back_enabled.set(enabled);
    }
    fn set_text_input_configuration(&mut self, configuration: TextInputConfiguration) {
        self.0.ime_configuration.replace(configuration);
        self.0.schedule_frame();
    }
    fn text_input_state_changed(&self, _: TextInputStateChange) {
        self.0.schedule_frame();
    }
    fn hide_soft_keyboard(&self) {
        self.0.dismiss_keyboard();
    }
    fn show_soft_keyboard(&self) {
        self.0.ime_show.set(true);
        self.0.schedule_frame();
    }
}
