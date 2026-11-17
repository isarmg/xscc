use anyhow::{Context as _, Result};
use std::{
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) struct Image {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

struct Job {
    image: Image,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: Arc<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
}

struct Completion {
    image: Image,
    result: Result<bool, String>,
}

pub(crate) struct Presenter {
    sender: Option<mpsc::SyncSender<Job>>,
    completed: mpsc::Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    free: Vec<Image>,
    allocated: usize,
    pending: usize,
    retry: bool,
    error: Option<String>,
    wake: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
}

impl Presenter {
    pub fn new() -> Result<Self> {
        let (sender, requests) = mpsc::sync_channel::<Job>(1);
        let (replies, completed) = mpsc::sync_channel(2);
        let wake: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = Arc::default();
        let worker_wake = wake.clone();
        let worker = thread::Builder::new().name("xscc-present".into()).spawn(move || {
            let mut blitter = None;
            let (mut since, mut count, mut total, mut longest) = (Instant::now(), 0u32, Duration::ZERO, Duration::ZERO);
            while let Ok(job) = requests.recv() {
                let start = Instant::now();
                let result = present(&job, &mut blitter).and_then(|shown| {
                    // Even Poll takes wgpu's command-indices lock. Progress maps
                    // here so recording/uploading the next scene can overlap present.
                    job.device.poll(wgpu::PollType::Poll)
                        .map(|_| shown)
                        .map_err(|error| format!("Poll Android presentation completion: {error}"))
                });
                let retry = !matches!(result, Ok(true));
                let elapsed = start.elapsed();
                total += elapsed;
                longest = longest.max(elapsed);
                count += 1;
                if since.elapsed() >= Duration::from_secs(1) {
                    log::info!("android_present_pipeline frames={} elapsed_ms={:.1} average_ms={:.3} max_ms={:.3}", count, since.elapsed().as_secs_f64()*1000., total.as_secs_f64()*1000./f64::from(count), longest.as_secs_f64()*1000.);
                    (since, count, total, longest) = (Instant::now(), 0, Duration::ZERO, Duration::ZERO);
                }
                if replies.send(Completion {image: job.image, result}).is_err() { break; }
                if retry && let Some(wake) = worker_wake.lock().as_ref() { wake(); }
            }
        }).context("Start Android presentation worker")?;
        Ok(Self {
            sender: Some(sender),
            completed,
            worker: Some(worker),
            free: Vec::with_capacity(2),
            allocated: 0,
            pending: 0,
            retry: false,
            error: None,
            wake,
        })
    }

    pub fn set_waker(&mut self, wake: Box<dyn Fn() + Send + Sync>) {
        *self.wake.lock() = Some(Arc::from(wake));
    }

    pub fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        self.wake.lock().clone()
    }

    pub fn recycle(&mut self, image: Image) {
        self.free.push(image);
    }

    fn complete(&mut self, completion: Completion) {
        self.pending -= 1;
        match completion.result {
            Ok(true) => {}
            Ok(false) => self.retry = true,
            Err(error) => {
                self.retry = true;
                self.error = Some(error);
            }
        }
        self.free.push(completion.image);
    }

    pub fn poll(&mut self) -> (bool, Option<String>) {
        while let Ok(completion) = self.completed.try_recv() {
            self.complete(completion);
        }
        (std::mem::take(&mut self.retry), self.error.take())
    }

    pub fn drain(&mut self) -> Result<()> {
        while self.pending > 0 {
            let completion = self
                .completed
                .recv()
                .context("Android presentation worker stopped before returning its images")?;
            self.complete(completion);
        }
        self.free.clear();
        self.allocated = 0;
        Ok(())
    }

    pub fn acquire(
        &mut self,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) -> Result<Image> {
        while let Ok(completion) = self.completed.try_recv() {
            self.complete(completion);
        }
        if self.free.is_empty() && self.allocated == 2 {
            let completion = self
                .completed
                .recv()
                .context("Android presentation worker stopped")?;
            self.complete(completion);
        }
        if let Some(image) = self.free.pop() {
            if image.texture.width() == config.width
                && image.texture.height() == config.height
                && image.texture.format() == config.format
            {
                return Ok(image);
            }
        } else {
            self.allocated += 1;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Android owned presentation image"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Image { texture, view })
    }

    pub fn submit(
        &mut self,
        image: Image,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface: Arc<wgpu::Surface<'static>>,
        config: wgpu::SurfaceConfiguration,
    ) -> Result<()> {
        let sender = self
            .sender
            .as_ref()
            .context("Android presenter is shutting down")?;
        sender
            .send(Job {
                image,
                device,
                queue,
                surface,
                config,
            })
            .map_err(|_| anyhow::anyhow!("Android presentation worker stopped"))?;
        self.pending += 1;
        Ok(())
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        if let Err(error) = self.drain() {
            log::error!("Android presentation drain failed: {error:#}");
        }
        self.sender.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::error!("Android presentation worker panicked");
        }
    }
}

fn present(
    job: &Job,
    blitter: &mut Option<(wgpu::TextureFormat, wgpu::util::TextureBlitter)>,
) -> Result<bool, String> {
    let frame = match job.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame) => frame,
        wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
            drop(frame);
            job.surface.configure(&job.device, &job.config);
            return Ok(false);
        }
        wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
            job.surface.configure(&job.device, &job.config);
            return Ok(false);
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            return Ok(false);
        }
        wgpu::CurrentSurfaceTexture::Validation => {
            return Err("Android surface texture validation failed".into());
        }
    };
    if frame.texture.width() != job.image.texture.width()
        || frame.texture.height() != job.image.texture.height()
    {
        drop(frame);
        return Ok(false);
    }
    let mut encoder = job
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Android presentation copy"),
        });
    if job.config.usage.contains(wgpu::TextureUsages::COPY_DST) {
        encoder.copy_texture_to_texture(
            job.image.texture.as_image_copy(),
            frame.texture.as_image_copy(),
            wgpu::Extent3d {
                width: job.config.width,
                height: job.config.height,
                depth_or_array_layers: 1,
            },
        );
    } else {
        if blitter
            .as_ref()
            .is_none_or(|(format, _)| *format != job.config.format)
        {
            *blitter = Some((
                job.config.format,
                wgpu::util::TextureBlitter::new(&job.device, job.config.format),
            ));
        }
        if let Some((_, blitter)) = blitter {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            blitter.copy(&job.device, &mut encoder, &job.image.view, &view);
        }
    }
    job.queue.submit([encoder.finish()]);
    job.queue.present(frame);
    Ok(true)
}
