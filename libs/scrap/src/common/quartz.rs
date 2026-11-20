use crate::{quartz, Frame, Pixfmt};
use std::io;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, TryLockError};

pub struct Capturer {
    inner: quartz::Capturer,
    frame: Arc<Mutex<Option<io::Result<quartz::Frame>>>>,
    saved_raw_data: Vec<u8>, // for faster compare and copy
}

impl Capturer {
    pub fn new(display: Display) -> io::Result<Capturer> {
        let frame = Arc::new(Mutex::new(None));

        let f = frame.clone();
        let inner = quartz::Capturer::new(
            display.0,
            display.width(),
            display.height(),
            quartz::PixelFormat::Argb8888,
            Default::default(),
            move |inner| {
                if let Ok(mut f) = f.lock() {
                    let previous = f.replace(inner);
                    drop(f);
                    drop(previous);
                }
            },
        )
        .map_err(|error| io::Error::other(format!("Could not start Quartz capture: {error:?}")))?;

        Ok(Capturer {
            inner,
            frame,
            saved_raw_data: Vec::new(),
        })
    }

    pub fn width(&self) -> usize {
        self.inner.width()
    }

    pub fn height(&self) -> usize {
        self.inner.height()
    }
}

impl crate::TraitCapturer for Capturer {
    fn frame<'a>(&'a mut self, _timeout_ms: std::time::Duration) -> io::Result<Frame<'a>> {
        if self.inner.is_stopped() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Quartz display stream stopped",
            ));
        }
        let frame = match self.frame.try_lock() {
            Ok(mut handle) => handle.take(),
            Err(TryLockError::WouldBlock) => return Err(io::ErrorKind::WouldBlock.into()),
            Err(TryLockError::Poisoned(..)) => {
                return Err(io::Error::other("Quartz frame lock was poisoned"))
            }
        };
        let frame = frame.ok_or(io::ErrorKind::WouldBlock)?;
        let mut frame = frame?;
        let (width, height) = (self.width(), self.height());
        frame.surface_to_bgra(width, height)?;
        crate::would_block_if_equal(&mut self.saved_raw_data, &frame)?;
        Ok(Frame::PixelBuffer(PixelBuffer {
            frame,
            data: PhantomData,
            width,
            height,
        }))
    }
}

pub struct PixelBuffer<'a> {
    frame: quartz::Frame,
    data: PhantomData<&'a [u8]>,
    width: usize,
    height: usize,
}

impl<'a> crate::TraitPixelBuffer for PixelBuffer<'a> {
    fn data(&self) -> &[u8] {
        &*self.frame
    }

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn stride(&self) -> Vec<usize> {
        let mut v = Vec::new();
        v.push(self.frame.stride());
        v
    }

    fn pixfmt(&self) -> Pixfmt {
        Pixfmt::BGRA
    }
}

pub struct Display(quartz::Display);

impl Display {
    pub fn primary() -> io::Result<Display> {
        Ok(Display(quartz::Display::primary()))
    }

    pub fn all() -> io::Result<Vec<Display>> {
        Ok(quartz::Display::online()
            .map_err(|error| {
                io::Error::other(format!("Could not list Quartz displays: {error:?}"))
            })?
            .into_iter()
            .map(Display)
            .collect())
    }

    pub fn width(&self) -> usize {
        self.0.width()
    }

    pub fn height(&self) -> usize {
        self.0.height()
    }

    pub fn scale(&self) -> f64 {
        self.0.scale()
    }

    pub fn name(&self) -> String {
        self.0.id().to_string()
    }

    pub fn is_online(&self) -> bool {
        self.0.is_online()
    }

    pub fn origin(&self) -> (i32, i32) {
        let o = self.0.bounds().origin;
        (o.x as _, o.y as _)
    }

    pub fn is_primary(&self) -> bool {
        self.0.is_primary()
    }
}
