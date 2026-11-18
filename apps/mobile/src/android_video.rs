use crate::session_layout::Rect;
use gpui::{AndroidApp, Bounds, DevicePixels, Global, ObjectFit, point, px, size};
use jni::{JValue, JavaVM, jni_sig, jni_str, objects::JObject, sys};
use librustdesk::gpui_session::VideoSurface;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI64, Ordering},
};
use tokio::sync::Notify;

pub(crate) struct VideoApp(pub AndroidApp);
impl Global for VideoApp {}

static NEXT_OWNER: AtomicI64 = AtomicI64::new(1);
static SURFACE: Mutex<Option<Registration>> = Mutex::new(None);

struct Registration {
    owner: i64,
    wake: Arc<Notify>,
    surface: Option<Arc<VideoSurface>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    bounds: [i32; 4],
    fps: i32,
}

impl Placement {
    pub(crate) fn new(
        rect: Rect,
        (width, height): (u32, u32),
        scale: f32,
        fps: i32,
    ) -> Option<Self> {
        if rect.width <= 0. || rect.height <= 0. || width == 0 || height == 0 {
            return None;
        }
        let image = ObjectFit::Contain.get_bounds(
            Bounds::new(
                point(px(rect.x), px(rect.y)),
                size(px(rect.width), px(rect.height)),
            ),
            size(
                DevicePixels(i32::try_from(width).ok()?),
                DevicePixels(i32::try_from(height).ok()?),
            ),
        );
        let physical = |point| (f32::from(point) * scale).round() as i32;
        let left = physical(image.left());
        let top = physical(image.top());
        Some(Self {
            bounds: [
                left,
                top,
                physical(image.right()) - left,
                physical(image.bottom()) - top,
            ],
            fps,
        })
    }
}

pub(crate) struct Layer {
    app: Option<AndroidApp>,
    wake: Arc<Notify>,
    request: Option<(i64, usize, Placement)>,
}

impl Layer {
    pub(crate) fn new(app: Option<AndroidApp>, wake: Arc<Notify>) -> Self {
        Self {
            app,
            wake,
            request: None,
        }
    }

    pub(crate) fn place(&mut self, display: usize, placement: Placement) {
        if self.app.is_none() {
            return;
        }
        let owner = match self.request {
            Some((_, old_display, old_placement))
                if old_display == display && old_placement == placement =>
            {
                return;
            }
            Some((owner, old_display, _)) if old_display == display => owner,
            _ => {
                self.clear();
                let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
                let previous = SURFACE.lock().unwrap().replace(Registration {
                    owner,
                    wake: self.wake.clone(),
                    surface: None,
                });
                if let Some(surface) = previous.and_then(|previous| previous.surface) {
                    surface.invalidate();
                }
                owner
            }
        };
        self.request = Some((owner, display, placement));
        self.place_on_main_thread(owner, placement.bounds, placement.fps);
    }

    pub(crate) fn target(&self) -> Option<(usize, Arc<VideoSurface>)> {
        let (owner, display, _) = self.request?;
        let slot = SURFACE.lock().unwrap();
        let registration = slot.as_ref().filter(|entry| entry.owner == owner)?;
        registration
            .surface
            .as_ref()
            .map(|surface| (display, surface.clone()))
    }

    pub(crate) fn clear(&mut self) {
        let Some((owner, _, _)) = self.request.take() else {
            return;
        };
        let surface = {
            let mut slot = SURFACE.lock().unwrap();
            if slot.as_ref().is_some_and(|entry| entry.owner == owner) {
                slot.take().and_then(|entry| entry.surface)
            } else {
                None
            }
        };
        if let Some(surface) = surface {
            surface.invalidate();
        }
        self.place_on_main_thread(owner, [0; 4], 0);
    }

    fn place_on_main_thread(&self, owner: i64, [x, y, width, height]: [i32; 4], fps: i32) {
        let Some(app) = self.app.clone() else {
            return;
        };
        let activity_app = app.clone();
        app.run_on_java_main_thread(Box::new(move || {
            let vm = unsafe { JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| {
                let raw: sys::jobject = activity_app.activity_as_ptr().cast();
                let activity = unsafe { env.as_cast_raw::<JObject>(&raw)? };
                let result = env.call_method(
                    &activity,
                    jni_str!("placeRemoteVideoSurface"),
                    jni_sig!("(JIIIII)V"),
                    &[
                        JValue::Long(owner),
                        JValue::Int(x),
                        JValue::Int(y),
                        JValue::Int(width),
                        JValue::Int(height),
                        JValue::Int(fps),
                    ],
                );
                if result.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                result.map(|_| ())
            });
            if let Err(error) = result {
                log::trace!("Cannot update Android video Surface bounds: {error}");
            }
        }));
    }
}

#[unsafe(no_mangle)]
extern "system" fn Java_org_xscc_gpui_RemoteVideoLayer_nativeSurfaceChanged(
    env: *mut sys::JNIEnv,
    _: sys::jclass,
    owner: sys::jlong,
    surface: sys::jobject,
    display_limit: sys::jint,
) {
    let (previous, wake) = {
        let mut slot = SURFACE.lock().unwrap();
        let Some(entry) = slot.as_mut().filter(|entry| entry.owner == owner) else {
            return;
        };
        let wake = entry.wake.clone();
        let next = if env.is_null() || surface.is_null() {
            None
        } else {
            let notify = wake.clone();
            // Java passes the Surface of the registered holder on its UI thread.
            unsafe {
                VideoSurface::from_java(
                    env.cast(),
                    surface.cast(),
                    display_limit,
                    Box::new(move || notify.notify_one()),
                )
            }
            .map(Arc::new)
        };
        (std::mem::replace(&mut entry.surface, next), wake)
    };
    if let Some(surface) = previous {
        surface.invalidate();
    }
    wake.notify_one();
}
