//! Servidor no macOS: captura por event tap (teclado e mouse) e ícone na barra de menus com as máquinas.
//! Mesma interface de `linux.rs`; a captura e o cursor ficam em `mousex_platform_macos::tap`.
//!
//! Atalhos (engolidos, como no Windows), com a troca Cmd↔Ctrl ligada na config (é o padrão): Cmd+Option+Shift+1…9
//! salta de máquina, F14 (o Scroll Lock do Mac) prende o cursor, e Cmd+Option+Shift+Home é a emergência.
//! Notebook sem tecla Home não tem como fazer a emergência: use um teclado externo com Home.
pub use super::common::{Button, Event, Hotkey, TrayCommand, TrayStatus};
use super::{
    InputGuard,
    common::{Latch, Mods, button_from},
};
use crate::{
    client_tray,
    config::{Config, data_dir},
};
use mousex_core::{
    i18n::{Lang, lang},
    keymap,
    proto::MonitorInfo,
};
use mousex_platform_macos::{
    Injector, Input, declare_user_activity,
    status_item::{self, Row},
    tap, windows_wheel,
};
use std::{
    io,
    sync::{LazyLock, Mutex, OnceLock, mpsc},
    thread,
    time::Duration,
};

pub const EDITOR: &str = "mousex-editor";

/// Tempo entre as tentativas quando o macOS ainda não deu permissão para o tap.
const RETRY: Duration = Duration::from_secs(3);
/// Atalhos de salto: Cmd+Option+Shift+1…9, como no Windows.
const JUMP_LIMIT: usize = 9;
// Ids das linhas do menu. Os saltos usam o próprio número (1…9).
const PAUSE: usize = 100;
const LOCK: usize = 101;
const RECONNECT: usize = 102;
const ARRANGE: usize = 103;
const LOG: usize = 104;
const ABOUT: usize = 105;

/// Canal dos eventos do servidor: o menu da barra manda os comandos por ele, como a bandeja do Linux faz.
static EVENTS: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
/// Último status publicado pelo servidor: o menu da barra mostra.
static STATUS: Mutex<Option<TrayStatus>> = Mutex::new(None);
/// Injetor só para soltar no Mac as teclas que estavam apertadas aqui quando o controle saiu.
static INJECTOR: LazyLock<Mutex<Injector>> = LazyLock::new(|| Mutex::new(Injector::default()));

pub fn start_input() -> io::Result<(InputGuard, mpsc::Receiver<Event>)> {
    let (tx, rx) = mpsc::channel();
    let _ = EVENTS.set(tx.clone());
    // Um cursor preso por uma execução que caiu não pode continuar preso: solta antes de tudo.
    tap::set_suppressed(false);
    let remap = remap_modifiers();
    thread::Builder::new().name("mousex-entrada".into()).spawn(move || capture(tx, remap))?;
    Ok((InputGuard {}, rx))
}

/// Config ausente (ainda não instalado) vale o padrão: a troca de modificadores fica ligada.
fn remap_modifiers() -> bool {
    Config::load(&data_dir()).is_none_or(|config| config.remap_modifiers)
}

/// Roda o tap; se o macOS recusar (permissão ainda não dada), tenta de novo sem derrubar o agente.
fn capture(tx: mpsc::Sender<Event>, remap: bool) {
    if !tap::listen_access() {
        crate::log("Input Monitoring is not granted: allow mousex-agent in Privacy & Security > Input Monitoring");
        tap::request_listen_access();
    }
    let mut warned = false;
    loop {
        let mut capture = Capture::new(tx.clone(), remap);
        match tap::run(move |input| capture.input(input)) {
            Ok(()) => return,
            Err(error) => {
                if !warned {
                    crate::log(&format!("keyboard and mouse capture unavailable: {error}"));
                    warned = true;
                }
                thread::sleep(RETRY);
            }
        }
    }
}

/// Traduz a entrada do Mac para os eventos do servidor. Mesma regra do hook do Windows: o que vai para a
/// outra máquina é publicado e engolido aqui quando o controle está fora.
struct Capture {
    tx: mpsc::Sender<Event>,
    mods: Mods,
    latch: Latch,
    remap: bool,
}

impl Capture {
    fn new(tx: mpsc::Sender<Event>, remap: bool) -> Self {
        Self { tx, mods: Mods::default(), latch: Latch::default(), remap }
    }

    /// `true` quando o evento não deve chegar às apps daqui.
    fn input(&mut self, input: Input) -> bool {
        let suppressed = tap::suppressed();
        match input {
            Input::Key { keycode, down } => match keymap::mac_key_to_win(keycode, self.remap) {
                Some((scancode, extended)) => self.key(scancode, extended, down) || suppressed,
                None => suppressed,
            },
            Input::Button { button, down } => {
                if let Some(button) = button_from(button) {
                    self.send(Event::Button { button, down });
                }
                suppressed
            }
            Input::Pointer { x, y, dx, dy } => {
                if suppressed {
                    if (dx, dy) != (0, 0) {
                        self.send(Event::Delta { dx: dx as i32, dy: dy as i32 });
                    }
                } else {
                    self.send(Event::Move { x: x.round() as i32, y: y.round() as i32 });
                }
                suppressed
            }
            Input::Scroll { vertical, horizontal } => {
                let (dx, dy) = windows_wheel(vertical, horizontal);
                if dy != 0 {
                    self.send(Event::Wheel { delta: dy, horizontal: false });
                }
                if dx != 0 {
                    self.send(Event::Wheel { delta: dx, horizontal: true });
                }
                suppressed
            }
            Input::TapRestarted => {
                crate::log("macOS had turned the input capture off; re-enabled it");
                false
            }
        }
    }

    /// Tecla já em scancode do Windows. Atalho e tecla engolida não chegam ao servidor como tecla.
    fn key(&mut self, scancode: u16, extended: bool, down: bool) -> bool {
        let _ = self.mods.key(scancode, extended, down);
        let (swallow, hotkey) = self.latch.key(&self.mods, scancode, extended, down);
        if let Some(hotkey) = hotkey {
            self.send(Event::Hotkey(hotkey));
            return true;
        }
        if !swallow {
            self.send(Event::Key { scancode, extended, down });
        }
        swallow
    }

    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

pub fn set_suppress(on: bool) -> bool {
    tap::set_suppressed(on)
}

pub fn release_if_pending() {}

pub fn park_cursor() -> (i32, i32) {
    tap::park()
}

pub fn set_cursor(x: i32, y: i32) {
    tap::warp(f64::from(x), f64::from(y));
}

/// Solta no Mac a tecla que estava apertada aqui quando o controle foi para a outra máquina.
pub fn release_key(scancode: u16, extended: bool) {
    let Some(keycode) = keymap::win_to_mac_key(scancode, extended, remap_modifiers()) else { return };
    if let Ok(mut injector) = INJECTOR.lock() {
        let _ = injector.key(keycode, false);
    }
}

pub fn keep_awake() {
    if !declare_user_activity() {
        crate::log("macOS refused to keep the display awake");
    }
}

pub fn set_monitor_positions(_positions: &[(String, i32, i32)]) -> io::Result<()> {
    Err(io::Error::other("macOS keeps its own screen arrangement (System Settings > Displays)"))
}

pub fn home_monitors() -> Vec<MonitorInfo> {
    crate::platform::local_monitors()
}

/// No Mac nenhuma barreira precisa de ajuste: a captura vê todas as bordas, como no Windows.
pub fn set_neighbors(_layout: &mousex_core::ScreenLayout, _home: &str) {}

pub fn set_tray_status(status: TrayStatus) {
    if let Ok(mut last) = STATUS.lock() {
        *last = Some(status);
    }
}

pub fn open_log() {
    client_tray::open_log();
}

/// Ícone da barra de menus do servidor (thread principal, bloqueia): máquinas, pausar, prender, reconectar,
/// editar telas, log e Sobre, os mesmos itens da bandeja do Windows e do Linux.
pub fn run_menu_bar() {
    let current = lang();
    status_item::run_menu(move || menu_rows(current), move |id| pick(id, current));
}

fn menu_rows(lang: Lang) -> Vec<Row> {
    let status = STATUS.lock().ok().and_then(|last| last.clone()).unwrap_or_default();
    let mut rows = Vec::new();
    for (index, (name, connected, version)) in status.machines.iter().enumerate() {
        let version = version.as_deref().map(|v| format!("   v{v}")).unwrap_or_default();
        let title = format!("{} {name}{version}", if *connected { "●" } else { "○" });
        match (*connected, index < JUMP_LIMIT) {
            (true, true) => rows.push(Row::Action { title, id: index + 1 }),
            _ => rows.push(Row::Label(title)),
        }
    }
    if !rows.is_empty() {
        rows.push(Row::Separator);
    }
    let pause = if status.paused { lang.pick("Retomar", "Resume") } else { lang.pick("Pausar", "Pause") };
    rows.push(Row::Action { title: pause.to_string(), id: PAUSE });
    let lock = lang.pick("Prender cursor neste computador", "Lock cursor to this computer");
    let lock = if status.locked { format!("✓ {lock}") } else { lock.to_string() };
    rows.push(Row::Action { title: lock, id: LOCK });
    rows.push(Row::Action { title: lang.pick("Reconectar", "Reconnect").to_string(), id: RECONNECT });
    rows.push(Row::Separator);
    rows.push(Row::Action { title: lang.pick("Arrumar telas…", "Arrange screens…").to_string(), id: ARRANGE });
    rows.push(Row::Action { title: client_tray::log_label(lang).to_string(), id: LOG });
    rows.push(Row::Separator);
    rows.push(Row::Action { title: client_tray::version_label(), id: ABOUT });
    rows
}

fn pick(id: usize, lang: Lang) {
    let command = match id {
        ABOUT => {
            return status_item::show_about(client_tray::about_title(lang), &client_tray::about_text(lang));
        }
        PAUSE => TrayCommand::Pause,
        LOCK => TrayCommand::LockCursor,
        RECONNECT => TrayCommand::Reconnect,
        ARRANGE => TrayCommand::Arrange,
        LOG => TrayCommand::ShowLog,
        jump @ 1..=JUMP_LIMIT => TrayCommand::Jump(jump as u8),
        _ => return,
    };
    if let Some(events) = EVENTS.get() {
        let _ = events.send(Event::Tray(command));
    }
}
