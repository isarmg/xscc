use anyhow::{Context as _, bail};
use gpui::PlatformTextSystem;
use gpui_wgpu::CosmicTextSystem;
use std::{borrow::Cow, collections::BTreeSet, ffi::CStr, sync::Arc};

/// Load the system's Latin, simplified/traditional Chinese and emoji fallback faces.
/// This slice deliberately does not claim complete Android font fallback coverage.
pub(crate) fn text_system() -> anyhow::Result<Arc<CosmicTextSystem>> {
    let mut paths = BTreeSet::new();
    // NDK font matching is available at our minimum API level 29. The font paths
    // remain valid only until AFont_close, so copy each before releasing it.
    unsafe {
        let matcher = ndk_sys::AFontMatcher_create();
        if matcher.is_null() {
            bail!("Android could not create a system font matcher");
        }
        for locale in [c"en", c"zh-Hans", c"zh-Hant"] {
            ndk_sys::AFontMatcher_setLocales(matcher, locale.as_ptr());
            for weight in [400, 700] {
                ndk_sys::AFontMatcher_setStyle(matcher, weight, false);
                let text: Vec<u16> = "GPUI 原生界面触摸滚动測試 😀".encode_utf16().collect();
                let mut offset = 0;
                while offset < text.len() {
                    let mut length = 0;
                    let font = ndk_sys::AFontMatcher_match(
                        matcher,
                        c"sans-serif".as_ptr(),
                        text[offset..].as_ptr(),
                        (text.len() - offset) as u32,
                        &mut length,
                    );
                    if font.is_null() || length == 0 {
                        if !font.is_null() {
                            ndk_sys::AFont_close(font);
                        }
                        ndk_sys::AFontMatcher_destroy(matcher);
                        bail!("Android system font matching failed");
                    }
                    let path = ndk_sys::AFont_getFontFilePath(font);
                    if !path.is_null() {
                        paths.insert(CStr::from_ptr(path).to_string_lossy().into_owned());
                    }
                    ndk_sys::AFont_close(font);
                    offset += length as usize;
                }
            }
        }
        ndk_sys::AFontMatcher_destroy(matcher);
    }
    let system = Arc::new(CosmicTextSystem::new_without_system_fonts("Roboto"));
    for path in paths {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("Cannot read Android system font {path}"))?;
        system.add_fonts(vec![Cow::Owned(bytes)])?;
    }
    if system.all_font_names().is_empty() {
        bail!("Android system font database is empty");
    }
    Ok(system)
}
