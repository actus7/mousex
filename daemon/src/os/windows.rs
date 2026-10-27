use super::InputGuard;
use std::{io, sync::mpsc};

pub use mousex_native::{Button, Event, Hotkey, TrayCommand, TrayStatus};

pub const EDITOR: &str = "mousex-editor.exe";

pub fn start_input() -> io::Result<(InputGuard, mpsc::Receiver<Event>)> {
    let (hooks, events) = mousex_native::Hooks::start()?;
    if let Err(error) = mousex_native::start_tray() {
        crate::log(&format!("tray icon unavailable: {error}"));
    }
    Ok((InputGuard { _hooks: hooks }, events))
}

pub fn set_suppress(on: bool) -> bool {
    mousex_native::Hooks::set_suppress(on);
    true
}

pub fn release_if_pending() {}

pub fn park_cursor() -> (i32, i32) {
    mousex_native::park_cursor()
}

pub fn set_cursor(x: i32, y: i32) {
    mousex_native::set_cursor(x, y);
}

pub fn release_key(scancode: u16, extended: bool) {
    use std::sync::{Mutex, OnceLock};
    static INJECTOR: OnceLock<Mutex<mousex_native::Injector>> = OnceLock::new();
    let injector = INJECTOR.get_or_init(|| Mutex::new(mousex_native::Injector::default()));
    if let Ok(mut injector) = injector.lock() {
        let _ = injector.send(Event::Key { scancode, extended, down: false });
    }
}

pub fn keep_awake() {
    mousex_native::keep_awake();
}

pub fn set_monitor_positions(positions: &[(String, i32, i32)]) -> io::Result<()> {
    mousex_native::set_monitor_positions(positions)
}

pub fn home_monitors() -> Vec<mousex_core::proto::MonitorInfo> {
    crate::platform::local_monitors()
}

pub fn set_tray_status(status: TrayStatus) {
    mousex_native::set_tray_status(status);
}

pub fn open_log() {
    let _ = std::process::Command::new("notepad.exe").arg(crate::config::data_dir().join("mousex.log")).spawn();
}
