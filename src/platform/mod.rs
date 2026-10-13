#[cfg(windows)]
pub mod windows;

use crate::input::session::InputSink;
use std::io;

pub fn clipboard_backend() -> io::Result<Box<dyn crate::clipboard::Backend>> {
    #[cfg(windows)]
    {
        crate::clipboard::native::create()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "此系统尚未实现剪贴板",
        ))
    }
}

pub fn input_sink() -> io::Result<Box<dyn InputSink>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::input_simulation::WindowsInput))
    }

    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "input receiver is not implemented for this OS",
        ))
    }
}
