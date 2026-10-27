//! Linux server host: InputCapture barriers plus a tray icon.
use super::InputGuard;
use crate::config::data_dir;
use mousex_core::{keymap, proto::MonitorInfo};
use mousex_platform_linux::{Captured, release_at, release_pending, set_holding, start_server_capture};
use std::{
    io,
    sync::{Mutex, mpsc},
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

// Same variants as Windows so the server can match both. Not all of them are produced here.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key { scancode: u16, extended: bool, down: bool },
    Button { button: Button, down: bool },
    Move { x: i32, y: i32 },
    Delta { dx: i32, dy: i32 },
    Wheel { delta: i16, horizontal: bool },
    DesktopSwitch,
    Tray(TrayCommand),
    DisplayChange,
    Hotkey(Hotkey),
    SessionLocked,
    HooksRestored(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    Jump(u8),
    ToggleLock,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Arrange,
    ShowLog,
    Pause,
    LockCursor,
    Reconnect,
    Jump(u8),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayStatus {
    pub machines: Vec<(String, bool)>,
    pub active: bool,
    pub paused: bool,
    pub locked: bool,
    pub portuguese: bool,
}

pub const EDITOR: &str = "mousex-editor";

static ZONES: Mutex<Vec<MonitorInfo>> = Mutex::new(Vec::new());

pub fn start_input() -> io::Result<(InputGuard, mpsc::Receiver<Event>)> {
    let capture = start_server_capture(data_dir().join("input-capture-token"))?;
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

fn button_from(value: u8) -> Option<Button> {
    Some(match value {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::X1,
        4 => Button::X2,
        _ => return None,
    })
}

#[derive(Default)]
struct Mods {
    bits: u8,
}

impl Mods {
    fn key(&mut self, scancode: u16, extended: bool, down: bool) -> bool {
        let bit = match (scancode, extended) {
            (0x1D, false) => 1,
            (0x1D, true) => 2,
            (0x38, false) => 4,
            (0x38, true) => 8,
            (0x2A, _) => 16,
            (0x36, _) => 32,
            _ => 0,
        };
        if bit != 0 {
            if down {
                self.bits |= bit;
            } else {
                self.bits &= !bit;
            }
            return false;
        }
        down && (scancode, extended) == (0x47, true) && self.all_held()
    }

    fn all_held(&self) -> bool {
        self.bits & 0b11 != 0 && self.bits & 0b1100 != 0 && self.bits & 0b11_0000 != 0
    }

    fn hotkey(&self, scancode: u16, extended: bool) -> Option<Hotkey> {
        match (scancode, extended) {
            (0x46, false) => Some(Hotkey::ToggleLock),
            (0x02..=0x0A, false) if self.all_held() => Some(Hotkey::Jump((scancode - 1) as u8)),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Latch {
    held: u16,
}

impl Latch {
    fn bit(scancode: u16, extended: bool) -> Option<u16> {
        match (scancode, extended) {
            (0x02..=0x0A, false) => Some(1 << (scancode - 2)),
            (0x46, false) => Some(1 << 9),
            _ => None,
        }
    }

    fn key(&mut self, modifiers: &Mods, scancode: u16, extended: bool, down: bool) -> (bool, Option<Hotkey>) {
        let Some(bit) = Self::bit(scancode, extended) else { return (false, None) };
        if !down {
            let swallowed = self.held & bit != 0;
            self.held &= !bit;
            return (swallowed, None);
        }
        if self.held & bit != 0 {
            return (true, None);
        }
        match modifiers.hotkey(scancode, extended) {
            Some(hotkey) => {
                self.held |= bit;
                (true, Some(hotkey))
            }
            None => (false, None),
        }
    }
}
