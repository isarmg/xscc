use super::IosWindowState;
use block2::RcBlock;
use objc2::AnyThread;
use objc2_foundation::NSError;
use objc2_ui_kit::{
    UIInterfaceOrientation, UIInterfaceOrientationMask, UIWindowSceneGeometryPreferencesIOS,
};
use std::ptr::NonNull;

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
        window.session_presentation.set(Some(previous));
        UIInterfaceOrientationMask::Landscape
    } else {
        window.session_presentation.set(None);
        original.unwrap_or_else(|| window.view_controller.supportedInterfaceOrientations())
    };
    window.view_controller.setNeedsStatusBarAppearanceUpdate();
    window
        .view_controller
        .setNeedsUpdateOfHomeIndicatorAutoHidden();
    window
        .view_controller
        .setNeedsUpdateOfSupportedInterfaceOrientations();
    if let Some(scene) = scene {
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
    window.force_next_frame.set(true);
    window.view.setNeedsLayout();
}
