//! X11 (ou XWayland): telas pelo RandR e injeção pelo XTest.
use super::{Backend, Display, other};
use crate::{Rect, Wheel, x11_button};
use std::io;
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{self, ConnectionExt as _},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

/// O X11 numera teclas como o evdev + 8.
const KEYCODE_OFFSET: u16 = 8;

pub(super) struct X11 {
    pub(super) conn: RustConnection,
    pub(super) root: xproto::Window,
    screen: usize,
}

impl X11 {
    pub(super) fn connect() -> io::Result<Self> {
        let (conn, screen) = x11rb::connect(None).map_err(other)?;
        let root = conn.setup().roots.get(screen).ok_or_else(|| other("X11 screen missing"))?.root;
        Ok(Self { conn, root, screen })
    }

    pub(super) fn displays(&self) -> io::Result<Vec<Display>> {
        let reply = self.conn.randr_get_monitors(self.root, true).map_err(other)?.reply().map_err(other)?;
        let mut displays = Vec::with_capacity(reply.monitors.len());
        for monitor in reply.monitors {
            let id = match self.conn.get_atom_name(monitor.name).map_err(other)?.reply() {
                Ok(name) => String::from_utf8_lossy(&name.name).into_owned(),
                Err(_) => monitor.name.to_string(),
            };
            let rect = Rect {
                x: monitor.x.into(),
                y: monitor.y.into(),
                width: monitor.width.into(),
                height: monitor.height.into(),
            };
            displays.push(Display { id, rect });
        }
        if displays.is_empty() {
            // Servidor sem RandR 1.5 (Xvfb antigo): a tela inteira como um monitor só.
            let root = &self.conn.setup().roots[self.screen];
            let rect = Rect { x: 0, y: 0, width: root.width_in_pixels.into(), height: root.height_in_pixels.into() };
            displays.push(Display { id: "screen".into(), rect });
        }
        Ok(displays)
    }

    /// Zera o contador de ociosidade do X11 (protetor de tela e bloqueio por inatividade).
    pub(super) fn reset_idle(&self) -> io::Result<()> {
        self.conn.force_screen_saver(xproto::ScreenSaver::RESET).map_err(other)?;
        self.conn.flush().map_err(other)
    }
}

pub(super) struct XTest {
    x11: X11,
    wheel: Wheel,
}

impl XTest {
    pub(super) fn new() -> io::Result<Self> {
        let x11 = X11::connect()?;
        x11.conn.xtest_get_version(2, 2).map_err(other)?.reply().map_err(other)?;
        Ok(Self { x11, wheel: Wheel::default() })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> io::Result<()> {
        self.x11.conn.xtest_fake_input(kind, detail, CURRENT_TIME, self.x11.root, x, y, 0).map_err(other)?;
        self.x11.conn.flush().map_err(other)
    }

    fn click(&self, button: u8, times: i32) -> io::Result<()> {
        for _ in 0..times {
            self.fake(xproto::BUTTON_PRESS_EVENT, button, 0, 0)?;
            self.fake(xproto::BUTTON_RELEASE_EVENT, button, 0, 0)?;
        }
        Ok(())
    }
}

impl Backend for XTest {
    fn key(&mut self, evdev: u16, down: bool) -> io::Result<()> {
        let keycode = u8::try_from(evdev + KEYCODE_OFFSET).map_err(|_| other("key outside the X11 range"))?;
        self.fake(if down { xproto::KEY_PRESS_EVENT } else { xproto::KEY_RELEASE_EVENT }, keycode, 0, 0)
    }

    fn motion(&mut self, x: i32, y: i32) -> io::Result<()> {
        let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
        // detail 0: posição absoluta na raiz.
        self.fake(xproto::MOTION_NOTIFY_EVENT, 0, clamp(x), clamp(y))
    }

    fn button(&mut self, button: u8, down: bool) -> io::Result<()> {
        let button = x11_button(button).ok_or_else(|| other("unknown mouse button"))?;
        self.fake(if down { xproto::BUTTON_PRESS_EVENT } else { xproto::BUTTON_RELEASE_EVENT }, button, 0, 0)
    }

    /// Botões 4/5 (cima/baixo) e 6/7 (esquerda/direita), um clique por dente.
    fn scroll(&mut self, dx: i32, dy: i32) -> io::Result<()> {
        let (nx, ny) = self.wheel.notches(dx, dy);
        self.click(if ny > 0 { 4 } else { 5 }, ny.abs())?;
        self.click(if nx > 0 { 7 } else { 6 }, nx.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Só com um X11 de verdade (a CI roda sob `xvfb-run`); sem DISPLAY, não há o que testar.
    fn x11() -> Option<X11> {
        std::env::var_os("DISPLAY")?;
        Some(X11::connect().expect("DISPLAY definido, mas o X11 não conecta"))
    }

    #[test]
    fn randr_lista_ao_menos_uma_tela() {
        let Some(x11) = x11() else { return };
        let displays = x11.displays().expect("RandR");
        assert!(!displays.is_empty());
        assert!(displays.iter().all(|d| d.rect.width > 0 && d.rect.height > 0), "{displays:?}");
    }

    #[test]
    fn xtest_move_o_cursor_e_aperta_tecla() {
        if x11().is_none() {
            return;
        }
        let mut xtest = XTest::new().expect("XTest");
        // Consultas pela mesma conexão: o X11 só garante a ordem dentro de uma conexão.
        let pointer = |x11: &X11| {
            let reply = x11.conn.query_pointer(x11.root).unwrap().reply().unwrap();
            (reply.root_x, reply.root_y)
        };
        let pressed = |x11: &X11, keycode: u8| {
            let keys = x11.conn.query_keymap().unwrap().reply().unwrap().keys;
            keys[keycode as usize / 8] & (1 << (keycode % 8)) != 0
        };
        xtest.motion(123, 45).expect("motion");
        assert_eq!(pointer(&xtest.x11), (123, 45));
        const KEY_A: u16 = 30;
        xtest.key(KEY_A, true).expect("press");
        assert!(pressed(&xtest.x11, 38), "A (evdev 30) é o keycode 38 no X11");
        xtest.key(KEY_A, false).expect("release");
        assert!(!pressed(&xtest.x11, 38));
    }
}
