//! Tipos e atalhos que o servidor recebe do sistema, iguais no Linux e no Mac (o Windows tem os seus, em
//! `mousex_native`). Os scancodes são os do Windows: o servidor fala só essa linguagem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

pub fn button_from(value: u8) -> Option<Button> {
    Some(match value {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::X1,
        4 => Button::X2,
        _ => return None,
    })
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
    /// Máquinas da esquerda para a direita (numeração dos atalhos), se estão conectadas e a versão do MouseX delas.
    pub machines: Vec<(String, bool, Option<String>)>,
    pub active: bool,
    pub paused: bool,
    pub locked: bool,
    pub portuguese: bool,
}

/// Estado dos modificadores (scancodes do Windows): o atalho de emergência e os saltos dependem dele.
#[derive(Default)]
pub struct Mods {
    bits: u8,
}

impl Mods {
    pub fn key(&mut self, scancode: u16, extended: bool, down: bool) -> bool {
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

    pub fn hotkey(&self, scancode: u16, extended: bool) -> Option<Hotkey> {
        match (scancode, extended) {
            (0x46, false) => Some(Hotkey::ToggleLock),
            (0x02..=0x0A, false) if self.all_held() => Some(Hotkey::Jump((scancode - 1) as u8)),
            _ => None,
        }
    }
}

/// Teclas de atalho engolidas: a soltura só é engolida se o aperto foi, e a repetição não dispara de novo.
#[derive(Default)]
pub struct Latch {
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

    pub fn key(&mut self, modifiers: &Mods, scancode: u16, extended: bool, down: bool) -> (bool, Option<Hotkey>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn held(mods: &mut Mods, keys: &[u16]) {
        for &scancode in keys {
            mods.key(scancode, false, true);
        }
    }

    #[test]
    fn salto_so_com_ctrl_alt_shift_e_numero() {
        let mut mods = Mods::default();
        held(&mut mods, &[0x1D, 0x38]);
        assert_eq!(mods.hotkey(0x03, false), None, "falta o Shift");
        held(&mut mods, &[0x2A]);
        assert_eq!(mods.hotkey(0x03, false), Some(Hotkey::Jump(2)), "2 salta para a segunda máquina");
    }

    #[test]
    fn emergencia_e_ctrl_alt_shift_home() {
        let mut mods = Mods::default();
        held(&mut mods, &[0x1D, 0x38, 0x2A]);
        assert!(mods.key(0x47, true, true));
        assert!(!mods.key(0x47, false, true), "Home do teclado numérico não é a emergência");
    }

    #[test]
    fn scroll_lock_some_no_mesmo_par_do_aperto() {
        let mods = Mods::default();
        let mut latch = Latch::default();
        assert_eq!(latch.key(&mods, 0x46, false, true), (true, Some(Hotkey::ToggleLock)));
        assert_eq!(latch.key(&mods, 0x46, false, true), (true, None), "repetição não dispara de novo");
        assert_eq!(latch.key(&mods, 0x46, false, false), (true, None), "a soltura também é engolida");
        assert_eq!(latch.key(&mods, 0x46, false, false), (false, None), "depois do par, volta a passar");
    }
}
