mod capture;
mod clipboard;
mod desktop;
mod eis;
mod x11;

pub use capture::{Captured, ServerCapture, Zone, release_at, release_pending, set_holding, start_server_capture};

pub use clipboard::{Clip, Clipboard, clipboard_seq};
pub use desktop::{ask_yes_no, keep_awake, lock_screen, notify};

use crate::Rect;
use std::{collections::BTreeSet, io, path::PathBuf};

/// Uma tela como o X11 (ou o XWayland) a vê, em pixels globais.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub id: String,
    pub rect: Rect,
}

/// Telas pelo RandR. Vazio sem X11 (sem DISPLAY, ou Wayland sem XWayland).
pub fn displays() -> Vec<Display> {
    x11::X11::connect().and_then(|x| x.displays()).unwrap_or_default()
}

/// Sessão Wayland: o XTest só alcançaria as janelas do XWayland, então a injeção vai pelo portal.
pub fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

trait Backend: Send {
    fn key(&mut self, evdev: u16, down: bool) -> io::Result<()>;
    /// Posição global, na mesma referência de `displays()`.
    fn motion(&mut self, x: i32, y: i32) -> io::Result<()>;
    /// `button` é o do protocolo (0 esquerdo…4 avançar).
    fn button(&mut self, button: u8, down: bool) -> io::Result<()>;
    /// Deltas do Windows: 120 por dente, positivo = cima/direita.
    fn scroll(&mut self, dx: i32, dy: i32) -> io::Result<()>;
    /// Telas em uso (as de `displays()`), para quem precisa converter posições.
    fn set_screens(&mut self, _screens: &[Rect]) {}
}

/// Injeta teclado e mouse e lembra o que está pressionado, para soltar tudo numa queda.
pub struct Injector {
    backend: Box<dyn Backend>,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u8>,
}

impl Injector {
    pub fn x11() -> io::Result<Self> {
        Ok(Self::with(Box::new(x11::XTest::new()?)))
    }

    /// Portal RemoteDesktop + libei. O sistema pede permissão uma vez; o token fica em `token_file`.
    /// Erra logo se não houver portal com ConnectToEIS (versão 2); a permissão é pedida em segundo plano.
    pub fn portal(token_file: PathBuf) -> io::Result<Self> {
        Ok(Self::with(Box::new(eis::Eis::start(token_file)?)))
    }

    fn with(backend: Box<dyn Backend>) -> Self {
        Self { backend, keys: BTreeSet::new(), buttons: BTreeSet::new() }
    }

    pub fn key(&mut self, evdev: u16, down: bool) -> io::Result<()> {
        if down {
            self.keys.insert(evdev);
        } else {
            self.keys.remove(&evdev);
        }
        self.backend.key(evdev, down)
    }

    pub fn move_to(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.backend.motion(x, y)
    }

    pub fn button(&mut self, button: u8, down: bool) -> io::Result<()> {
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.backend.button(button, down)
    }

    pub fn scroll(&mut self, dx: i32, dy: i32) -> io::Result<()> {
        self.backend.scroll(dx, dy)
    }

    /// Telas de `displays()` em uso: no Wayland, base para converter posições para as do compositor.
    pub fn set_screens(&mut self, screens: &[Rect]) {
        self.backend.set_screens(screens);
    }

    /// Solta tudo que foi injetado (nenhuma tecla presa após queda).
    pub fn release_all(&mut self) {
        for button in std::mem::take(&mut self.buttons) {
            let _ = self.backend.button(button, false);
        }
        for key in std::mem::take(&mut self.keys) {
            let _ = self.backend.key(key, false);
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
