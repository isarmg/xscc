use super::*;
use gpui::{Corners, ImageId, RenderImage};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Params {
    bounds: PodBounds,
    content_mask: PodBounds,
    rounded_bounds: PodBounds,
    corner_radii: [f32; 4],
    opacity: f32,
    grayscale: u32,
    pad: [u32; 2],
}

impl Params {
    pub(super) fn new(
        image_bounds: Bounds<ScaledPixels>,
        bounds: Bounds<ScaledPixels>,
        mask: Bounds<ScaledPixels>,
        radii: Corners<ScaledPixels>,
        opacity: f32,
        grayscale: bool,
    ) -> Self {
        Self {
            bounds: image_bounds.into(),
            content_mask: mask.intersect(&bounds).into(),
            rounded_bounds: bounds.into(),
            corner_radii: [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ],
            opacity,
            grayscale: grayscale as u32,
            pad: [0; 2],
        }
    }
}

pub(super) struct Frame<'a> {
    pub image: &'a RenderImage,
    pub params: Params,
}

#[derive(Default)]
pub(super) struct Nv12Surfaces {
    frames: Vec<Uploaded>,
}

struct Uploaded {
    size: Size<DevicePixels>,
    image: Option<ImageId>,
    y: wgpu::Texture,
    uv: wgpu::Texture,
    params: wgpu::Buffer,
    bindings: wgpu::BindGroup,
}

pub(super) fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let plane = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nv12_surfaces_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Params>() as u64),
                },
                count: None,
            },
            plane(1),
            plane(2),
        ],
    })
}

impl Uploaded {
    fn new(resources: &WgpuResources, size: Size<DevicePixels>) -> Result<Self> {
        let (width, height) = (size.width.0 as u32, size.height.0 as u32);
        let device = &resources.device;
        anyhow::ensure!(
            width > 0
                && height > 0
                && width.max(height) <= device.limits().max_texture_dimension_2d,
            "NV12 video dimensions exceed the rendering device limit"
        );
        let texture = |label, width, height, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let y = texture("nv12_luma", width, height, wgpu::TextureFormat::R8Unorm);
        let uv = texture(
            "nv12_chroma",
            width.div_ceil(2),
            height.div_ceil(2),
            wgpu::TextureFormat::Rg8Unorm,
        );
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nv12_geometry"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nv12_planes"),
            layout: &resources.bind_group_layouts.nv12,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &y.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &uv.create_view(&Default::default()),
                    ),
                },
            ],
        });
        Ok(Self {
            size,
            image: None,
            y,
            uv,
            params,
            bindings,
        })
    }
}

impl Nv12Surfaces {
    pub(super) fn prepare<'a>(
        &mut self,
        resources: &WgpuResources,
        frames: impl ExactSizeIterator<Item = Frame<'a>>,
    ) -> Result<()> {
        // Retain only the current scene's surfaces, never a cache per image or resolution.
        self.frames.truncate(frames.len());
        for (index, frame) in frames.enumerate() {
            let size = frame.image.size(0);
            if index == self.frames.len() {
                self.frames.push(Uploaded::new(resources, size)?);
            } else if self.frames[index].size != size {
                self.frames[index] = Uploaded::new(resources, size)?;
            }
            let uploaded = &mut self.frames[index];
            if uploaded.image != Some(frame.image.id) {
                let (y, uv) = frame
                    .image
                    .nv12_planes()
                    .context("NV12 surface has no planes")?;
                for (texture, bytes, width, height, row) in [
                    (
                        &uploaded.y,
                        y,
                        size.width.0 as u32,
                        size.height.0 as u32,
                        size.width.0 as u32,
                    ),
                    (
                        &uploaded.uv,
                        uv,
                        (size.width.0 as u32).div_ceil(2),
                        (size.height.0 as u32).div_ceil(2),
                        (size.width.0 as u32).div_ceil(2) * 2,
                    ),
                ] {
                    // write_texture snapshots the borrowed planes and orders the upload
                    // before this submission's reads, including reuse after the prior draw.
                    resources.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                uploaded.image = Some(frame.image.id);
            }
            resources
                .queue
                .write_buffer(&uploaded.params, 0, bytemuck::bytes_of(&frame.params));
        }
        Ok(())
    }

    pub(super) fn draw(
        &self,
        resources: &WgpuResources,
        range: Range<usize>,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        pass.set_pipeline(&resources.pipelines.nv12);
        pass.set_bind_group(0, &resources.globals_bind_group, &[]);
        for frame in &self.frames[range] {
            pass.set_bind_group(1, &frame.bindings, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        )
    }

    fn draw(
        renderer: &mut WgpuHeadlessRenderer,
        image: &RenderImage,
        params: Params,
    ) -> Result<()> {
        let size = image.size(0);
        renderer.check_gpu_errors()?;
        renderer.ensure_render_target(size)?;
        renderer.core.nv12.prepare(
            &renderer.core.resources,
            std::iter::once(Frame { image, params }),
        )?;
        let resources = &renderer.core.resources;
        resources.queue.write_buffer(
            &resources.globals_buffer,
            0,
            bytemuck::bytes_of(&GlobalParams {
                viewport_size: [size.width.0 as f32, size.height.0 as f32],
                premultiplied_alpha: 0,
                pad: 0,
            }),
        );
        let mut encoder = resources.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &renderer.render_target.as_ref().unwrap().view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            renderer.core.nv12.draw(resources, 0..1, &mut pass);
        }
        resources.queue.submit([encoder.finish()]);
        Ok(())
    }

    #[test]
    #[ignore = "manual upload/draw timing; not Android codec, swapchain or scanout performance"]
    fn compare_full_hd_upload_and_draw() -> Result<()> {
        use gpui::{AtlasKey, ContentMask, PlatformAtlas, PolychromeSprite, RenderImageParams};
        use std::{borrow::Cow, time::Instant};

        let mut renderer = WgpuHeadlessRenderer::new()?;
        eprintln!("GPU: {:?}", renderer.context.adapter.get_info());
        let size = size(DevicePixels(1920), DevicePixels(1080));
        let rect = bounds(0., 0., 1920., 1080.);
        let params = Params::new(rect, rect, rect, Corners::default(), 1., false);
        let nv12: Arc<[u8]> = vec![128; 1920 * 1080 * 3 / 2].into();
        let bgra = vec![128; 1920 * 1080 * 4];
        let key = |id| {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(id),
                frame_index: 0,
            })
        };
        let mut previous = key(0);
        let tile = renderer
            .core
            .atlas
            .get_or_insert_with(previous.clone(), &mut || {
                Ok(Some((size, Cow::Borrowed(&bgra))))
            })?
            .unwrap();
        let mut scene = Scene::default();
        scene.insert_primitive(PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds: rect,
            content_mask: ContentMask { bounds: rect },
            corner_radii: Corners::default(),
            tile,
        });
        scene.finish();
        for nv12_path in [false, true, true, false] {
            let mut samples = Vec::new();
            let total = Instant::now();
            for frame in 0..140 {
                let start = Instant::now();
                if nv12_path {
                    let image = RenderImage::from_nv12(1920, 1080, nv12.clone())?;
                    draw(&mut renderer, &image, params)?;
                } else {
                    let next = key(frame + 1);
                    anyhow::ensure!(renderer.core.atlas.replace(
                        &previous,
                        next.clone(),
                        size,
                        &bgra
                    )?);
                    previous = next;
                    renderer.render(&scene, size)?;
                }
                if frame >= 20 {
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
                // Bound the fixture's GPU backlog to two draws instead of timing an unbounded queue.
                if frame % 2 == 1 {
                    renderer
                        .core
                        .resources
                        .device
                        .poll(wgpu::PollType::wait_indefinitely())?;
                }
            }
            renderer.check_gpu_errors()?;
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}: CPU enqueue p50={:.3}ms p95={:.3}ms; 140 draws including GPU drain={:.3}s",
                if nv12_path { "NV12" } else { "BGRA atlas" },
                samples[60],
                samples[114],
                total.elapsed().as_secs_f64()
            );
        }
        Ok(())
    }

    #[test]
    fn nv12_gpu_render_preserves_color_order_and_parent_clipping() -> Result<()> {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        let mut pixels = [235, 235, 81, 81, 145, 145, 41, 41].repeat(2);
        pixels.extend_from_slice(&[128, 128, 90, 240, 54, 34, 240, 110]);
        let image = RenderImage::from_nv12(8, 2, pixels)?;
        let image_bounds = bounds(0., 0., 8., 2.);
        let params = Params::new(
            image_bounds,
            image_bounds,
            bounds(1., 0., 6., 2.),
            Corners::default(),
            1.,
            false,
        );
        draw(&mut renderer, &image, params)?;
        let actual = renderer.read_image()?;
        let expected = [
            [0, 0, 0],
            [255, 255, 255],
            [254, 0, 0],
            [254, 0, 0],
            [0, 255, 1],
            [0, 255, 1],
            [0, 0, 255],
            [0, 0, 0],
        ];
        for y in 0..2 {
            for x in 0..8 {
                let pixel = actual.get_pixel(x, y).0;
                for channel in 0..3 {
                    assert!(
                        (pixel[channel] as i32 - expected[x as usize][channel]).abs() <= 2,
                        "{x},{y}: {pixel:?}"
                    );
                }
                assert_eq!(pixel[3], 255);
            }
        }
        Ok(())
    }

    #[test]
    fn nv12_gpu_replacement_resize_and_static_redraw_keep_the_last_owned_image() -> Result<()> {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        for (width, height, value) in [(8, 2, 16), (8, 2, 235), (3, 3, 16), (3, 3, 235)] {
            let mut pixels = vec![value; width * height];
            pixels.resize(
                pixels.len() + width.div_ceil(2) * 2 * height.div_ceil(2),
                128,
            );
            let image = RenderImage::from_nv12(width as u32, height as u32, pixels)?;
            let rect = bounds(0., 0., width as f32, height as f32);
            let params = Params::new(rect, rect, rect, Corners::default(), 1., false);
            draw(&mut renderer, &image, params)?;
            // A cursor-only redraw reuses the uploaded image without another decoder output.
            draw(&mut renderer, &image, params)?;
            drop(image);
            let actual = renderer.read_image()?;
            let expected = if value == 16 { 0 } else { 255 };
            assert!(
                actual
                    .pixels()
                    .all(|pixel| pixel.0 == [expected, expected, expected, 255])
            );
        }
        renderer
            .core
            .nv12
            .prepare(&renderer.core.resources, std::iter::empty())?;
        Ok(())
    }
}
