#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

use crate::input::session::InputSink;
use std::io;

pub fn clipboard_backend() -> io::Result<Box<dyn crate::clipboard::Backend>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        crate::clipboard::native::create()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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
    #[cfg(target_os = "macos")]
    {
        macos::input_simulation::MacInput::new().map(|sink| Box::new(sink) as Box<dyn InputSink>)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "input receiver is not implemented for this OS",
        ))
    }
}
