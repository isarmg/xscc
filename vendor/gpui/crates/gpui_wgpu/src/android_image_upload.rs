use crate::image_upload_layout::layout;
use anyhow::{Context as _, Result, ensure};

#[derive(Default)]
pub(crate) struct ImageUpload {
    pool: Option<Pool>,
}

struct Pool {
    extent: (u32, u32),
    belt: wgpu::util::StagingBelt,
}

impl ImageUpload {
    pub(crate) fn write(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: wgpu::TexelCopyTextureInfo<'_>,
        extent: wgpu::Extent3d,
        bytes: &[u8],
    ) -> Result<bool> {
        let layout = layout(extent.width, extent.height).context("Invalid image upload layout")?;
        ensure!(
            extent.depth_or_array_layers == 1
                && u64::from(layout.row_bytes) * u64::from(extent.height) == bytes.len() as u64,
            "Image upload requires complete packed pixels"
        );
        if layout.size > device.limits().max_buffer_size {
            return Ok(false);
        }
        // Poll without waiting so completed maps return buffers to the belt before allocation.
        device
            .poll(wgpu::PollType::Poll)
            .context("Poll image upload completion")?;
        let dimensions = (extent.width, extent.height);
        if self
            .pool
            .as_ref()
            .is_none_or(|pool| pool.extent != dimensions)
        {
            // Submitted commands retain old buffers; changing resolution does not retain a cache per size.
            self.pool = Some(Pool {
                extent: dimensions,
                belt: wgpu::util::StagingBelt::new(device.clone(), 1024 * 1024),
            });
        }
        let pool = self
            .pool
            .as_mut()
            .expect("invariant: upload pool initialized");
        {
            let size = wgpu::BufferSize::new(layout.size).context("Empty image upload")?;
            let alignment = wgpu::BufferSize::new(256).expect("invariant: nonzero copy alignment");
            let staging = pool.belt.allocate(size, alignment);
            {
                let mut mapped = staging
                    .get_mapped_range_mut()
                    .context("Map image upload buffer")?;
                if layout.row_bytes == layout.padded_row {
                    mapped.copy_from_slice(bytes);
                } else {
                    for (row, source) in bytes.chunks_exact(layout.row_bytes as usize).enumerate() {
                        let start = row * layout.padded_row as usize;
                        mapped
                            .slice(start..start + source.len())
                            .copy_from_slice(source);
                    }
                }
            }
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: staging.buffer(),
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: staging.offset(),
                        bytes_per_row: Some(layout.padded_row),
                        rows_per_image: None,
                    },
                },
                target,
                extent,
            );
        }
        // Only GPU-completed buffers are re-mapped. Never recycle a buffer still being read.
        pool.belt.finish_and_recall_on_submit(encoder);
        Ok(true)
    }
}
