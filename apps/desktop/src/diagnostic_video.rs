use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub ready: Instant,
}

#[derive(Default)]
pub struct Mailbox {
    pub frame: Option<Frame>,
    pub produced: u64,
    pub replaced: u64,
    pub error: Option<String>,
}

pub struct Source {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Source {
    pub fn start(
        mailbox: Arc<Mutex<Mailbox>>,
        width: u32,
        height: u32,
        fps: u32,
        ivf: Option<&Path>,
    ) -> io::Result<Self> {
        let mut decoder = ivf.map(Ivf::open).transpose()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::Builder::new()
            .name("ui-validation-video".into())
            .spawn(move || {
                let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
                let mut next = Instant::now();
                let mut sequence = 0;
                let template = pattern(width, height, 0).rgba;
                while !stopping.load(Ordering::Relaxed) {
                    let frame = match &mut decoder {
                        Some(decoder) => match decoder.next() {
                            Ok(frame) => frame,
                            Err(error) => {
                                mailbox.lock().unwrap().error = Some(error.to_string());
                                break;
                            }
                        },
                        None => {
                            let mut rgba = template.clone();
                            let column = sequence as usize % width as usize * 4;
                            for row in rgba.chunks_exact_mut(width as usize * 4) {
                                row[column..column + 4].copy_from_slice(&[255, 255, 255, 255]);
                            }
                            Frame {
                                width,
                                height,
                                rgba,
                                ready: Instant::now(),
                            }
                        }
                    };
                    {
                        let mut state = mailbox.lock().unwrap();
                        state.produced += 1;
                        state.replaced += u64::from(state.frame.replace(frame).is_some());
                    }
                    sequence += 1;
                    next += interval;
                    if let Some(wait) = next.checked_duration_since(Instant::now()) {
                        thread::sleep(wait);
                    } else {
                        next = Instant::now();
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("video validation worker terminated unexpectedly");
        }
    }
}

fn pattern(width: u32, height: u32, sequence: u64) -> Frame {
    let mut rgba = vec![0; width as usize * height as usize * 4];
    for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = index as u32 % width;
        let y = index as u32 / width;
        pixel.copy_from_slice(&[
            (x * 255 / width) as u8,
            (y * 255 / height) as u8,
            if ((x / 32 + y / 32 + sequence as u32 / 5) & 1) == 0 {
                32
            } else {
                224
            },
            255,
        ]);
    }
    Frame {
        width,
        height,
        rgba,
        ready: Instant::now(),
    }
}

struct Ivf {
    file: File,
    width: u32,
    height: u32,
    #[cfg(all(feature = "xscc-codec", target_os = "linux"))]
    decoder: scrap::codec::Decoder,
}

impl Ivf {
    fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut header = [0; 32];
        file.read_exact(&mut header)?;
        if &header[..4] != b"DKIF" || header[4..8] != [0, 0, 32, 0] || &header[8..12] != b"VP90" {
            return Err(io::Error::other("expected a version-0 VP9 IVF file"));
        }
        let width = u32::from(u16::from_le_bytes([header[12], header[13]]));
        let height = u32::from(u16::from_le_bytes([header[14], header[15]]));
        if width == 0 || height == 0 || width > 3840 || height > 2160 {
            return Err(io::Error::other("IVF dimensions must be within 3840x2160"));
        }
        #[cfg(all(feature = "xscc-codec", target_os = "linux"))]
        return Ok(Self {
            file,
            width,
            height,
            decoder: scrap::codec::Decoder::new(scrap::CodecFormat::VP9, None),
        });
        #[cfg(not(all(feature = "xscc-codec", target_os = "linux")))]
        Err(io::Error::other(
            "VP9 replay requires --features xscc-codec on Linux",
        ))
    }

    fn packet(&mut self) -> io::Result<Vec<u8>> {
        let mut header = [0; 12];
        if self.file.read(&mut header[..1])? == 0 {
            self.file.seek(SeekFrom::Start(32))?;
            #[cfg(all(feature = "xscc-codec", target_os = "linux"))]
            {
                self.decoder = scrap::codec::Decoder::new(scrap::CodecFormat::VP9, None);
            }
            self.file.read_exact(&mut header[..1])?;
        }
        self.file.read_exact(&mut header[1..])?;
        let size = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if size == 0 || size > 16 * 1024 * 1024 {
            return Err(io::Error::other("invalid IVF packet size"));
        }
        let mut bytes = vec![0; size];
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn next(&mut self) -> io::Result<Frame> {
        let packet = self.packet()?;
        #[cfg(all(feature = "xscc-codec", target_os = "linux"))]
        {
            use base::message_proto::{EncodedVideoFrame, EncodedVideoFrames, video_frame};
            let encoded = video_frame::Union::Vp9s(EncodedVideoFrames {
                frames: vec![EncodedVideoFrame {
                    data: packet.into(),
                    ..Default::default()
                }],
                ..Default::default()
            });
            let mut rgb = scrap::ImageRgb::new(scrap::ImageFormat::ABGR, 1);
            let mut texture = scrap::ImageTexture::default();
            let available = self
                .decoder
                .handle_video_frame(&encoded, &mut rgb, &mut texture, &mut true, &mut None)
                .map_err(|e| io::Error::other(e.to_string()))?;
            if !available
                || rgb.w != self.width as usize
                || rgb.h != self.height as usize
                || rgb.raw.len() != self.width as usize * self.height as usize * 4
            {
                return Err(io::Error::other(
                    "decoder returned no frame or unexpected dimensions",
                ));
            }
            Ok(Frame {
                width: self.width,
                height: self.height,
                rgba: rgb.raw,
                ready: Instant::now(),
            })
        }
        #[cfg(not(all(feature = "xscc-codec", target_os = "linux")))]
        {
            let _ = (packet, self.width, self.height);
            Err(io::Error::other("xscc-codec is disabled"))
        }
    }
}
