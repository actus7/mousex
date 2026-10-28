//! Canal local de entrada: tamanho fixo, sem caminhos, strings ou comandos extensíveis.
use mousex_core::keymap;

pub const PACKET_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key { hid: u16, down: bool },
    Move { x: i32, y: i32 },
    Button { button: u8, down: bool },
    Wheel { dx: i32, dy: i32 },
    Release,
}

impl Input {
    pub fn encode(self) -> [u8; PACKET_SIZE] {
        let mut bytes = [0; PACKET_SIZE];
        bytes[0] = 1;
        let (op, down, a, b) = match self {
            Self::Key { hid, down } => (1, down, i32::from(hid), 0),
            Self::Move { x, y } => (2, false, x, y),
            Self::Button { button, down } => (3, down, i32::from(button), 0),
            Self::Wheel { dx, dy } => (4, false, dx, dy),
            Self::Release => (5, false, 0, 0),
        };
        bytes[1] = op;
        bytes[2] = u8::from(down);
        bytes[4..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..12].copy_from_slice(&b.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != PACKET_SIZE || bytes[0] != 1 || bytes[2] > 1 || bytes[3] != 0 {
            return None;
        }
        let a = i32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let b = i32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let down = bytes[2] == 1;
        Some(match bytes[1] {
            1 if b == 0 => {
                let hid = u16::try_from(a).ok()?;
                keymap::hid_to_win_scancode(hid)?;
                Self::Key { hid, down }
            }
            2 if !down => Self::Move { x: a, y: b },
            3 if b == 0 && (0..=4).contains(&a) => Self::Button { button: a as u8, down },
            4 if !down && i16::try_from(a).is_ok() && i16::try_from(b).is_ok() => Self::Wheel { dx: a, dy: b },
            5 if !down && a == 0 && b == 0 => Self::Release,
            _ => return None,
        })
    }
}

/// Erro ao consultar o marcador conta como agente presente: falha fechada.
pub fn helper_connects(agent_present: Result<bool, ()>, paired: bool) -> bool {
    paired && agent_present == Ok(false)
}

/// Executável sempre entre aspas; argumentos são constantes internas.
pub fn agent_command(exe: &str, mode: &str) -> Option<String> {
    if exe.is_empty() || exe.contains(['"', '\0']) || !matches!(mode, "--service" | "--logon") {
        return None;
    }
    Some(format!("\"{exe}\" {mode}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comandos_citam_caminho_e_recusam_argumentos_externos() {
        assert_eq!(
            agent_command(r"C:\MouseX\mousex-agent.exe", "--service").as_deref(),
            Some(r#""C:\MouseX\mousex-agent.exe" --service"#)
        );
        assert_eq!(
            agent_command(r"C:\Program Files\MouseX\mousex-agent.exe", "--logon").as_deref(),
            Some(r#""C:\Program Files\MouseX\mousex-agent.exe" --logon"#)
        );
        assert!(agent_command("a\"b", "--logon").is_none());
        assert!(agent_command("a\0b", "--service").is_none());
        assert!(agent_command("a", "--other").is_none());
    }

    #[test]
    fn handoff_exige_ausencia_confirmada_e_pareamento() {
        assert!(helper_connects(Ok(false), true));
        for present in [Ok(true), Err(())] {
            assert!(!helper_connects(present, true));
        }
        assert!(!helper_connects(Ok(false), false));
    }

    #[test]
    fn mensagens_validas_e_limites() {
        for input in [
            Input::Key { hid: 4, down: true },
            Input::Key { hid: 0xe0, down: false },
            Input::Move { x: i32::MIN, y: i32::MAX },
            Input::Button { button: 4, down: false },
            Input::Wheel { dx: -32768, dy: 32767 },
            Input::Release,
        ] {
            assert_eq!(Input::decode(&input.encode()), Some(input));
        }
    }

    #[test]
    fn recusa_truncados_opcodes_flags_e_payloads_invalidos() {
        let valid = Input::Key { hid: 4, down: true }.encode();
        for len in 0..PACKET_SIZE {
            assert!(Input::decode(&valid[..len]).is_none());
        }
        assert!(Input::decode(&[0; PACKET_SIZE + 1]).is_none());
        for (offset, value) in [(0, 2), (1, 99), (2, 2), (3, 1), (8, 1)] {
            let mut bad = valid;
            bad[offset] = value;
            assert!(Input::decode(&bad).is_none());
        }
        for bad in [
            Input::Key { hid: 0xffff, down: true },
            Input::Button { button: 5, down: true },
            Input::Wheel { dx: 32768, dy: 0 },
        ] {
            assert!(Input::decode(&bad.encode()).is_none());
        }
    }
}
