use super::IosWindowState;
use block2::RcBlock;
use objc2::{AnyThread, MainThreadOnly};
use objc2_foundation::NSError;
use objc2_ui_kit::{
    UIInterfaceOrientation, UIInterfaceOrientationMask, UIView, UIWindowSceneGeometryPreferencesIOS,
};
use std::ptr::NonNull;

#[derive(Clone, Copy)]
pub(super) struct Presentation {
    original_orientation: UIInterfaceOrientationMask,
    orientation: UIInterfaceOrientationMask,
}

impl IosWindowState {
    pub(super) fn session_orientation(&self) -> Option<UIInterfaceOrientationMask> {
        self.session_presentation
            .get()
            .map(|presentation| presentation.orientation)
    }

    pub(in crate::ios) fn session_frame_rate(&self) -> Option<f32> {
        self.session_presentation
            .get()
            .map(|_| self.window.screen().maximumFramesPerSecond() as f32)
    }
}

pub(super) fn set_enabled(window: &IosWindowState, enabled: bool) {
    let original = window.session_presentation.get();
    if enabled == original.is_some() {
        return;
    }
    let scene = window.window.windowScene();
    let orientation = if enabled {
        let previous = scene
            .as_ref()
            .map(|scene| {
                let orientation = scene.effectiveGeometry().interfaceOrientation();
                match orientation {
                    UIInterfaceOrientation::Portrait => UIInterfaceOrientationMask::Portrait,
                    UIInterfaceOrientation::PortraitUpsideDown => {
                        UIInterfaceOrientationMask::PortraitUpsideDown
                    }
                    UIInterfaceOrientation::LandscapeLeft => {
                        UIInterfaceOrientationMask::LandscapeLeft
                    }
                    UIInterfaceOrientation::LandscapeRight => {
                        UIInterfaceOrientationMask::LandscapeRight
                    }
                    _ => window.view_controller.supportedInterfaceOrientations(),
                }
            })
            .unwrap_or_else(|| window.view_controller.supportedInterfaceOrientations());
        window.session_presentation.set(Some(Presentation {
            original_orientation: previous,
            orientation: UIInterfaceOrientationMask::Landscape,
        }));
        UIInterfaceOrientationMask::Landscape
    } else {
        window.session_presentation.set(None);
        original
            .map(|presentation| presentation.original_orientation)
            .unwrap_or_else(|| window.view_controller.supportedInterfaceOrientations())
    };
    if let Some(platform) = window.platform.upgrade() {
        platform.application.update_session_frame_rate();
    }
    let update_geometry = RcBlock::new(|| {
        window.view_controller.setNeedsStatusBarAppearanceUpdate();
        window
            .view_controller
            .setNeedsUpdateOfHomeIndicatorAutoHidden();
        window
            .view_controller
            .setNeedsUpdateOfSupportedInterfaceOrientations();
        if let Some(scene) = scene.as_ref() {
            let preferences = UIWindowSceneGeometryPreferencesIOS::initWithInterfaceOrientations(
                UIWindowSceneGeometryPreferencesIOS::alloc(),
                orientation,
            );
            let on_error = RcBlock::new(move |error: NonNull<NSError>| {
                // iPad multitasking may decline orientation changes; keep the actual scene's bounds.
                let error = unsafe { error.as_ref() };
                log::warn!(
                    "iOS declined session presentation geometry (enabled={enabled}, code={})",
                    error.code()
                );
            });
            scene.requestGeometryUpdateWithPreferences_errorHandler(&preferences, Some(&on_error));
        } else {
            log::warn!("Cannot request iOS session orientation without a window scene");
        }
    });
    UIView::performWithoutAnimation(&update_geometry, window.window.mtm());
    window.force_next_frame.set(true);
    window.view.setNeedsLayout();
}

pub(super) fn rotate(window: &IosWindowState) {
    let Some(mut presentation) = window.session_presentation.get() else {
        return;
    };
    presentation.orientation = if presentation.orientation == UIInterfaceOrientationMask::Landscape
    {
        UIInterfaceOrientationMask::Portrait
    } else {
        UIInterfaceOrientationMask::Landscape
    };
    window.session_presentation.set(Some(presentation));
    let scene = window.window.windowScene();
    let update_geometry = RcBlock::new(|| {
        window
            .view_controller
            .setNeedsUpdateOfSupportedInterfaceOrientations();
        if let Some(scene) = scene.as_ref() {
            let preferences = UIWindowSceneGeometryPreferencesIOS::initWithInterfaceOrientations(
                UIWindowSceneGeometryPreferencesIOS::alloc(),
                presentation.orientation,
            );
            let on_error = RcBlock::new(move |error: NonNull<NSError>| {
                let error = unsafe { error.as_ref() };
                log::warn!(
                    "iOS declined remote session rotation (code={})",
                    error.code()
                );
            });
            scene.requestGeometryUpdateWithPreferences_errorHandler(&preferences, Some(&on_error));
        } else {
            log::warn!("Cannot rotate iOS remote session without a window scene");
        }
    });
    UIView::performWithoutAnimation(&update_geometry, window.window.mtm());
    window.force_next_frame.set(true);
    window.view.setNeedsLayout();
}
