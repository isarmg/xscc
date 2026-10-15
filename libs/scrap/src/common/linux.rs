use crate::{
    common::{wayland, TraitCapturer},
    Frame,
};
use std::{io, time::Duration};

pub enum Capturer {
    WAYLAND(wayland::Capturer),
}

impl Capturer {
    pub fn new(display: Display) -> io::Result<Capturer> {
        Ok(match display {
            Display::WAYLAND(d) => Capturer::WAYLAND(wayland::Capturer::new(d)?),
        })
    }

    pub fn width(&self) -> usize {
        match self {
            Capturer::WAYLAND(d) => d.width(),
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Capturer::WAYLAND(d) => d.height(),
        }
    }
}

impl TraitCapturer for Capturer {
    fn frame<'a>(&'a mut self, timeout: Duration) -> io::Result<Frame<'a>> {
        match self {
            Capturer::WAYLAND(d) => d.frame(timeout),
        }
    }
}

pub enum Display {
    WAYLAND(wayland::Display),
}

impl Display {
    pub fn primary() -> io::Result<Display> {
        Ok(Display::WAYLAND(wayland::Display::primary()?))
    }

    // The portal session must be initialized before enumerating displays.
    pub fn all() -> io::Result<Vec<Display>> {
        Ok(wayland::Display::all()?
            .into_iter()
            .map(Display::WAYLAND)
            .collect())
    }

    pub fn width(&self) -> usize {
        match self {
            Display::WAYLAND(d) => d.width(),
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Display::WAYLAND(d) => d.height(),
        }
    }

    pub fn scale(&self) -> f64 {
        match self {
            Display::WAYLAND(d) => d.scale(),
        }
    }

    pub fn logical_width(&self) -> usize {
        match self {
            Display::WAYLAND(d) => d.logical_width(),
        }
    }

    pub fn logical_height(&self) -> usize {
        match self {
            Display::WAYLAND(d) => d.logical_height(),
        }
    }

    pub fn origin(&self) -> (i32, i32) {
        match self {
            Display::WAYLAND(d) => d.origin(),
        }
    }

    pub fn is_online(&self) -> bool {
        match self {
            Display::WAYLAND(d) => d.is_online(),
        }
    }

    pub fn is_primary(&self) -> bool {
        match self {
            Display::WAYLAND(d) => d.is_primary(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Display::WAYLAND(d) => d.name(),
        }
    }
}
