#[cfg(target_os = "android")]
use anyhow::{Context as _, Result, ensure};

#[cfg(target_os = "android")]
#[derive(Default)]
pub(crate) struct ImageUpload {
    pool: Option<Pool>,
}

#[cfg(target_os = "android")]
struct Pool {
    extent: (u32, u32),
    belt: wgpu::util::StagingBelt,
}

#[cfg(target_os = "android")]
impl ImageUpload {
    pub(crate) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Android image upload"),
        });
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
        pool.belt.finish_and_recall_on_submit(&encoder);
        queue.submit(std::iter::once(encoder.finish()));
        Ok(true)
    }
}

#[derive(Debug, PartialEq)]
struct Layout {
    row_bytes: u32,
    padded_row: u32,
    size: u64,
}

fn layout(width: u32, height: u32) -> Option<Layout> {
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = width.checked_mul(4)?;
    let padded_row = row_bytes.checked_add(255)? / 256 * 256;
    Some(Layout {
        row_bytes,
        padded_row,
        size: u64::from(padded_row).checked_mul(u64::from(height))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_and_custom_widths_have_valid_gpu_copy_rows() {
        assert_eq!(
            layout(1920, 1080),
            Some(Layout {
                row_bytes: 7680,
                padded_row: 7680,
                size: 8_294_400
            })
        );
        assert_eq!(
            layout(641, 480),
            Some(Layout {
                row_bytes: 2564,
                padded_row: 2816,
                size: 1_351_680
            })
        );
        assert_eq!(layout(3840, 2160).unwrap().size, 33_177_600);
    }

    #[test]
    fn empty_and_overflowing_copy_rows_are_rejected() {
        assert!(layout(0, 1080).is_none());
        assert!(layout(1920, 0).is_none());
        assert!(layout(u32::MAX, 1).is_none());
        assert!(layout(u32::MAX / 4, 1).is_none());
    }
}
