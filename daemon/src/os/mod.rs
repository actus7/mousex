//! What the server asks of the operating system. Windows uses the native hooks.
//! Linux captures the pointer through the InputCapture portal. macOS uses a CGEvent tap.
use std::io;
use std::sync::mpsc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod common;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

/// Kept alive for the process so the capture thread is not dropped.
pub struct InputGuard {
    #[cfg(windows)]
    _hooks: mousex_native::Hooks,
}

pub fn start_input() -> io::Result<(InputGuard, mpsc::Receiver<Event>)> {
    #[cfg(windows)]
    {
        windows::start_input()
    }
    #[cfg(target_os = "linux")]
    {
        linux::start_input()
    }
    #[cfg(target_os = "macos")]
    {
        macos::start_input()
    }
}
