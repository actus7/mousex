//! Plataforma Windows real: injeção via SendInput, monitores e clipboard Win32.
use crate::client::Platform;
use crate::clip::Clip;
use mousex_core::{keymap, proto::MonitorInfo};
use mousex_native::{Button, Event, Injector};

pub fn button_to_u8(button: Button) -> u8 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::X1 => 3,
        Button::X2 => 4,
    }
}

pub fn button_from_u8(value: u8) -> Option<Button> {
    Some(match value {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::X1,
        4 => Button::X2,
        _ => return None,
    })
}

/// Monitores em pixels físicos (DPI awareness per-monitor v2); escala 1.0 no Windows.
pub fn local_monitors() -> Vec<MonitorInfo> {
    mousex_native::monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|m| MonitorInfo {
            id: m.name,
            x: m.bounds.left,
            y: m.bounds.top,
            width: m.bounds.right - m.bounds.left,
            height: m.bounds.bottom - m.bounds.top,
            scale: 1.0,
        })
        .collect()
}

pub struct WinPlatform {
    injector: Injector,
    /// Origem de cada monitor, na mesma ordem do Hello enviado.
    origins: Vec<(i32, i32)>,
}

impl WinPlatform {
    pub fn new(monitors: &[MonitorInfo]) -> Self {
        let mut injector = Injector::default();
        // Processo anterior encerrado à força (atualização) não roda Drop: solta modificadores que possam ter ficado presos.
        for (scancode, extended) in [
            (0x1D, false),
            (0x1D, true),
            (0x38, false),
            (0x38, true),
            (0x2A, false),
            (0x36, false),
            (0x5B, true),
            (0x5C, true),
        ] {
            let _ = injector.send(Event::Key { scancode, extended, down: false });
        }
        Self { injector, origins: monitors.iter().map(|m| (m.x, m.y)).collect() }
    }
}

// Falhas de injeção (UIPI, desktop seguro) são ignoradas: nada a fazer além de não travar.
impl Platform for WinPlatform {
    fn key(&mut self, hid: u16, down: bool) {
        if let Some((scancode, extended)) = keymap::hid_to_win_scancode(hid) {
            let _ = self.injector.send(Event::Key { scancode, extended, down });
        }
    }

    fn move_to(&mut self, monitor: usize, x: i32, y: i32) {
        if let Some(&(ox, oy)) = self.origins.get(monitor) {
            let _ = self.injector.send(Event::Move { x: ox + x, y: oy + y });
        }
    }

    fn button(&mut self, button: u8, down: bool) {
        if let Some(button) = button_from_u8(button) {
            let _ = self.injector.send(Event::Button { button, down });
        }
    }

    fn wheel(&mut self, dx: i32, dy: i32) {
        let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        if dy != 0 {
            let _ = self.injector.send(Event::Wheel { delta: clamp(dy), horizontal: false });
        }
        if dx != 0 {
            let _ = self.injector.send(Event::Wheel { delta: clamp(dx), horizontal: true });
        }
    }

    fn release_all(&mut self) {
        let _ = self.injector.release_all();
    }

    fn clipboard_seq(&self) -> u32 {
        mousex_native::clipboard_seq()
    }

    fn clipboard_read(&self) -> Option<Clip> {
        clipboard_read()
    }

    fn clipboard_write(&mut self, clip: Clip) {
        clipboard_write(&clip);
    }

    fn keep_awake(&mut self) {
        mousex_native::keep_awake();
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        local_monitors()
    }

    fn set_monitors(&mut self, monitors: &[MonitorInfo]) {
        self.origins = monitors.iter().map(|m| (m.x, m.y)).collect();
    }

    fn lock_screen(&mut self) {
        if let Err(error) = mousex_native::lock_workstation() {
            crate::log(&format!("failed to lock the screen: {error}"));
        }
    }
}

pub type Native = WinPlatform;

pub fn clipboard_read() -> Option<Clip> {
    match mousex_native::clipboard_read().ok().flatten()? {
        mousex_native::Clip::Text(text) => Some(Clip::Text(text)),
        mousex_native::Clip::Png(png) => Some(Clip::Png(png)),
        mousex_native::Clip::Files(paths) => Some(Clip::Files(paths)),
    }
}

pub fn clipboard_write(clip: &Clip) {
    let native = match clip {
        Clip::Text(text) => mousex_native::Clip::Text(text.clone()),
        Clip::Png(png) => mousex_native::Clip::Png(png.clone()),
        Clip::Files(paths) => mousex_native::Clip::Files(paths.clone()),
    };
    let _ = mousex_native::clipboard_write(&native);
}

/// Abre arquivos copiados com os direitos do usuário comum (o agente roda elevado).
pub fn as_standard_user<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    mousex_native::as_standard_user(f)
}

/// Pasta de recebidos: criada privada; se já existe, não pode ser link nem, com o agente elevado,
/// pertencer a outro dono que não Administradores (programa comum que a criou antes).
pub fn ensure_inbox(inbox: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    match std::fs::symlink_metadata(inbox) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = inbox.parent() {
                std::fs::create_dir_all(parent)?;
            }
            mousex_native::create_private_dir(inbox)
        }
        Err(error) => Err(error),
        Ok(meta) if meta.file_attributes() & REPARSE_POINT != 0 || !meta.is_dir() => {
            Err(std::io::Error::other("received-files folder is a link"))
        }
        Ok(_) if mousex_native::is_elevated() && !mousex_native::owned_by_admins(inbox)? => {
            Err(std::io::Error::other("received-files folder is not owned by Administrators"))
        }
        Ok(_) => Ok(()),
    }
}

pub fn free_space(path: &std::path::Path) -> Option<u64> {
    mousex_native::free_space(path).ok()
}

pub fn ask_yes_no(title: &str, text: &str) -> bool {
    mousex_native::ask_yes_no(title, text)
}

/// Preparação do processo antes de qualquer janela ou coordenada.
pub fn prepare() -> bool {
    mousex_native::init_dpi_awareness()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn botoes_ida_e_volta() {
        for button in [Button::Left, Button::Right, Button::Middle, Button::X1, Button::X2] {
            assert_eq!(button_from_u8(button_to_u8(button)), Some(button));
        }
        assert_eq!(button_from_u8(9), None);
    }
}
