#[cfg(not(windows))]
compile_error!(
    "Use the Windows target for this repository; the other platform has its own repository."
);
pub mod bluetooth;
pub mod clipboard;
pub mod consent;
pub mod desktop;
pub mod discovery;
pub mod input;
pub mod network;
pub mod platform;
pub mod transfer;
