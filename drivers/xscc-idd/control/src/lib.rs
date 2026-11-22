// SPDX-License-Identifier: MIT
//! Control transport for the independent Root\XsccIdd prototype.
//!
//! This library does not install a driver, select a product backend, change CCD
//! modes, or operate MTT devices. Driver inventory is not active-display proof.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(any(windows, test))]

#[cfg(windows)]
use std::io;
use std::{error, fmt};

mod protocol;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Owner;

pub use protocol::{Capabilities, ConnectorId, Mode, Monitor};
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    operation: &'static str,
    cause: Cause,
}

#[derive(Debug)]
enum Cause {
    #[cfg(windows)]
    Io(io::Error),
    Protocol(&'static str),
}

impl Error {
    fn protocol(operation: &'static str, message: &'static str) -> Self {
        Self {
            operation,
            cause: Cause::Protocol(message),
        }
    }

    #[cfg(windows)]
    fn io(operation: &'static str, cause: io::Error) -> Self {
        Self {
            operation,
            cause: Cause::Io(cause),
        }
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        match &self.cause {
            #[cfg(windows)]
            Cause::Io(error) => error.raw_os_error(),
            Cause::Protocol(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.operation)?;
        match &self.cause {
            #[cfg(windows)]
            Cause::Io(error) => error.fmt(f),
            Cause::Protocol(message) => f.write_str(message),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.cause {
            #[cfg(windows)]
            Cause::Io(error) => Some(error),
            Cause::Protocol(_) => None,
        }
    }
}
