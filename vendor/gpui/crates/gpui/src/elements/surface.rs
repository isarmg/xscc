use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    ObjectFit, Pixels, Style, StyleRefinement, Styled, Window,
};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2_core_foundation::CFRetained;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use refineable::Refineable;

/// A source of a surface's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceSource {
    /// An immutable, keyed-mutex shared BGRA texture.
    #[cfg(target_os = "windows")]
    Windows(WindowsSurface),
    /// A CoreVideo image buffer
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Surface(CFRetained<CVPixelBuffer>),
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl From<CFRetained<CVPixelBuffer>> for SurfaceSource {
    fn from(value: CFRetained<CVPixelBuffer>) -> Self {
        SurfaceSource::Surface(value)
    }
}

/// An immutable BGRA texture published by a D3D11 producer.
///
/// Experimental: the renderer imports this texture on every draw.
#[cfg(target_os = "windows")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsSurface {
    texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    width: u32,
    height: u32,
}

#[cfg(target_os = "windows")]
impl WindowsSurface {
    /// Retain a published, immutable shared texture.
    ///
    /// # Safety
    /// The producer must own the texture independently of decoder output slots,
    /// finish submitting its writes, Flush, and ReleaseSync(1) before this call.
    /// No caller may write this texture again or acquire its keyed mutex while
    /// any clone of this surface exists. The renderer repeatedly acquires and
    /// releases key 1; it does not return a mutable slot to the producer.
    /// BGRA pixels must be opaque SDR values (alpha 255 for every pixel).
    pub unsafe fn from_published_texture(
        texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    ) -> anyhow::Result<Self> {
        use windows::{
            Win32::Graphics::{
                Direct3D11::*,
                Dxgi::{Common::*, IDXGIKeyedMutex},
            },
            core::Interface,
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        anyhow::ensure!(desc.Width > 0 && desc.Height > 0, "empty surface");
        anyhow::ensure!(
            desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
                && desc.MipLevels == 1
                && desc.ArraySize == 1
                && desc.SampleDesc.Count == 1
                && desc.SampleDesc.Quality == 0
                && desc.Usage == D3D11_USAGE_DEFAULT
                && desc.CPUAccessFlags == 0
                && desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 != 0
                && desc.MiscFlags == D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
            "surface must be legacy keyed-mutex shared, single-slice BGRA8 DEFAULT shader resource"
        );
        let _: IDXGIKeyedMutex = texture.cast()?;
        Ok(Self {
            texture,
            width: desc.Width,
            height: desc.Height,
        })
    }

    /// The retained immutable producer texture; do not mutate it.
    pub fn texture(&self) -> &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D {
        &self.texture
    }

    /// Pixel dimensions of the source texture.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[cfg(target_os = "windows")]
impl From<WindowsSurface> for SurfaceSource {
    fn from(value: WindowsSurface) -> Self {
        Self::Windows(value)
    }
}

/// A surface element.
pub struct Surface {
    source: SurfaceSource,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

/// Create a new surface element.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
pub fn surface(source: impl Into<SurfaceSource>) -> Surface {
    Surface {
        source: source.into(),
        object_fit: ObjectFit::Contain,
        style: Default::default(),
    }
}

impl Surface {
    /// Set the object fit for the image.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        #[cfg_attr(
            not(any(target_os = "macos", target_os = "ios")),
            allow(unused_variables)
        )]
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        #[cfg_attr(
            not(any(target_os = "macos", target_os = "ios")),
            allow(unused_variables)
        )]
        window: &mut Window,
        _: &mut App,
    ) {
        match &self.source {
            #[cfg(target_os = "windows")]
            SurfaceSource::Windows(surface) => {
                let (width, height) = surface.dimensions();
                let size = crate::size(width.into(), height.into());
                window.paint_surface(self.object_fit.get_bounds(bounds, size), surface.clone());
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            SurfaceSource::Surface(surface) => {
                let size = crate::size(
                    CVPixelBufferGetWidth(surface).into(),
                    CVPixelBufferGetHeight(surface).into(),
                );
                let new_bounds = self.object_fit.get_bounds(bounds, size);
                // TODO: Add support for corner_radii
                window.paint_surface(new_bounds, surface.clone());
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

impl IntoElement for Surface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
