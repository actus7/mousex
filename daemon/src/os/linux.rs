//! Linux server host: InputCapture barriers plus a tray icon.
use super::{
    InputGuard,
    common::{Latch, Mods, button_from},
};
use crate::config::data_dir;
use mousex_core::{ScreenLayout, keymap, proto::MonitorInfo};
use mousex_platform_linux::{Area, Captured, release_at, release_pending, set_holding, start_server_capture};
use std::{
    io,
    sync::{Mutex, mpsc},
    thread,
};

pub use super::common::{Button, Event, Hotkey, TrayCommand, TrayStatus};

pub const EDITOR: &str = "mousex-editor";

static ZONES: Mutex<Vec<MonitorInfo>> = Mutex::new(Vec::new());

pub fn start_input() -> io::Result<(InputGuard, mpsc::Receiver<Event>)> {
    let capture = start_server_capture(data_dir().join("input-capture-token"), saved_neighbors())?;
    if let Ok(mut zones) = ZONES.lock() {
        *zones = capture
            .zones
            .iter()
            .map(|zone| MonitorInfo {
                id: zone.id.clone(),
                x: zone.x,
                y: zone.y,
                width: zone.width,
                height: zone.height,
                scale: 1.0,
            })
            .collect();
    }
    let (tx, rx) = mpsc::channel();
    let bridge = tx.clone();
    thread::Builder::new().name("mousex-ponte".into()).spawn(move || {
        let mut mods = Mods::default();
        let mut latch = Latch::default();
        while let Ok(captured) = capture.events.recv() {
            if let Some(event) = map_captured(captured, &mut mods, &mut latch)
                && bridge.send(event).is_err()
            {
                break;
            }
        }
    })?;
    if let Err(error) = crate::linux_tray::start(tx) {
        crate::log(&format!("tray icon unavailable: {error}"));
    }
    Ok((InputGuard {}, rx))
}

/// Telas dos outros computadores no layout salvo: as barreiras já nascem só nas bordas que encostam nelas.
fn saved_neighbors() -> Vec<Area> {
    let dir = data_dir();
    let Some(config) = crate::config::Config::load(&dir) else { return Vec::new() };
    neighbors(&crate::server::read_saved(&dir.join("layout.json")), &config.machine)
}

fn neighbors(layout: &ScreenLayout, home: &str) -> Vec<Area> {
    let others = layout.monitores.iter().filter(|m| m.machine != home);
    others.map(|m| Area { x: m.x, y: m.y, width: m.width, height: m.height }).collect()
}

/// Layout novo (lado trocado, editor, computador novo): as barreiras vão para as bordas que encostam agora.
pub fn set_neighbors(layout: &ScreenLayout, home: &str) {
    mousex_platform_linux::set_neighbors(neighbors(layout, home));
}

pub fn set_suppress(on: bool) -> bool {
    if on { set_holding(true) } else { true }
}

pub fn release_if_pending() {
    release_pending();
}

pub fn park_cursor() -> (i32, i32) {
    (0, 0)
}

pub fn set_cursor(x: i32, y: i32) {
    release_at(x, y);
}

pub fn release_key(_scancode: u16, _extended: bool) {}

pub fn keep_awake() {
    mousex_platform_linux::keep_awake();
}

pub fn set_monitor_positions(_positions: &[(String, i32, i32)]) -> io::Result<()> {
    Err(io::Error::other("this desktop keeps its own screen arrangement"))
}

pub fn home_monitors() -> Vec<MonitorInfo> {
    let zones = ZONES.lock().map(|zones| zones.clone()).unwrap_or_default();
    if zones.is_empty() { crate::platform::local_monitors() } else { zones }
}

/// Último status publicado pelo servidor: a bandeja mostra, e o editor de telas sabe quem está conectado.
static STATUS: Mutex<Option<TrayStatus>> = Mutex::new(None);

pub fn set_tray_status(status: TrayStatus) {
    if let Ok(mut last) = STATUS.lock() {
        *last = Some(status.clone());
    }
    crate::linux_tray::set_status(status);
}

/// Máquinas conectadas agora (a casa inclusa); vazio se o servidor não roda neste processo.
pub fn connected_machines() -> Vec<String> {
    let Ok(last) = STATUS.lock() else { return Vec::new() };
    let machines = last.as_ref().map(|status| status.machines.as_slice()).unwrap_or_default();
    machines.iter().filter(|(_, connected)| *connected).map(|(name, _)| name.clone()).collect()
}

pub fn open_log() {
    let _ = std::process::Command::new("xdg-open").arg(data_dir().join("mousex.log")).spawn();
}

fn map_captured(captured: Captured, mods: &mut Mods, latch: &mut Latch) -> Option<Event> {
    match captured {
        Captured::Note(line) => {
            crate::log(&line);
            None
        }
        Captured::Move { x, y } => Some(Event::Move { x, y }),
        Captured::Delta { dx, dy } => Some(Event::Delta { dx, dy }),
        Captured::Wheel { delta, horizontal } => Some(Event::Wheel { delta, horizontal }),
        Captured::SessionLocked => Some(Event::SessionLocked),
        // Same as a desktop switch on Windows: the key-ups will not come; forget them and go home.
        Captured::Released => Some(Event::DesktopSwitch),
        Captured::Button { button, down } => Some(Event::Button { button: button_from(button)?, down }),
        Captured::Key { evdev, down } => {
            let hid = keymap::evdev_to_hid(evdev)?;
            let (scancode, extended) = keymap::hid_to_win_scancode(hid)?;
            let _ = mods.key(scancode, extended, down);
            let (swallow, hotkey) = latch.key(mods, scancode, extended, down);
            if let Some(hotkey) = hotkey {
                return Some(Event::Hotkey(hotkey));
            }
            if swallow {
                return None;
            }
            Some(Event::Key { scancode, extended, down })
        }
    }
}
