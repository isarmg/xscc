use crate::video::Frame;
use slint::{Image, wgpu_30::wgpu};

pub struct Upload {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: Option<wgpu::Texture>,
    image: Option<Image>,
    pub rgba_import_checked: bool,
    pub planar_import_rejected: bool,
}
impl Upload {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            texture: None,
            image: None,
            rgba_import_checked: false,
            planar_import_rejected: false,
        }
    }
    pub fn frame(&mut self, frame: &Frame) -> Result<Image, String> {
        if self.texture.as_ref().is_none_or(|texture| {
            texture.width() != frame.width || texture.height() != frame.height
        }) {
            let descriptor = wgpu::TextureDescriptor {
                label: Some("xscc validation: CPU RGBA upload"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            };
            let texture = self.device.create_texture(&descriptor);
            let image = Image::try_from(texture.clone())
                .map_err(|e| format!("RGBA texture import: {e}"))?;
            let planar = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("xscc validation: planar Y texture capability check"),
                format: wgpu::TextureFormat::R8Unorm,
                ..descriptor
            });
            self.planar_import_rejected = Image::try_from(planar).is_err();
            self.rgba_import_checked = true;
            self.texture = Some(texture);
            self.image = Some(image);
        }
        let texture = self
            .texture
            .as_ref()
            .ok_or("texture initialization failed")?;
        self.queue.write_texture(
            texture.as_image_copy(),
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            texture.size(),
        );
        self.image
            .clone()
            .ok_or_else(|| "texture image initialization failed".into())
    }
}
