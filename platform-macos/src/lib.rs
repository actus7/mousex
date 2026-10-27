//! Backend macOS (cliente): injeção via CGEvent, monitores e clipboard. Keycodes são kVK_* nativos.
//! A lógica pura (modificadores, contagem de cliques) fica aqui e é testada em qualquer SO.

use std::time::{Duration, Instant};

/// Bits de `CGEventFlags` (iguais aos do macOS), mantidos aqui para teste fora do Mac.
pub const FLAG_CAPS: u64 = 0x0001_0000;
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_OPTION: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
pub const FLAG_NUMERIC_PAD: u64 = 0x0020_0000;
pub const FLAG_FN: u64 = 0x0080_0000;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const CLICK_SLOP: f64 = 5.0;

/// Bit de modificador de um keycode kVK_*, se for modificador.
pub fn modifier_flag(keycode: u16) -> Option<u64> {
    Some(match keycode {
        0x37 | 0x36 => FLAG_COMMAND,
        0x38 | 0x3C => FLAG_SHIFT,
        0x3A | 0x3D => FLAG_OPTION,
        0x3B | 0x3E => FLAG_CONTROL,
        0x39 => FLAG_CAPS,
        _ => return None,
    })
}

/// Flags que o teclado real manda junto com certas teclas: setas levam "teclado numérico" e "Fn";
/// Home/End/PgUp/PgDn/Delete e F-keys levam "Fn"; teclado numérico leva "teclado numérico".
/// Sem elas, apps tratam Shift+seta de forma errada (não selecionam).
pub fn key_flags(keycode: u16) -> u64 {
    match keycode {
        0x7B..=0x7E => FLAG_NUMERIC_PAD | FLAG_FN,
        0x72..=0x79 | 0x7A | 0x60..=0x6F | 0x40 | 0x4F | 0x50 | 0x5A => FLAG_FN,
        0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51..=0x59 | 0x5B | 0x5C | 0x5F => FLAG_NUMERIC_PAD,
        _ => 0,
    }
}

/// Eventos sintéticos não herdam o estado dos modificadores: cada evento leva as flags explicitamente.
#[derive(Debug, Default, Clone)]
pub struct Modifiers {
    held: std::collections::BTreeSet<u16>,
    caps: bool,
}

impl Modifiers {
    pub fn key(&mut self, keycode: u16, down: bool) {
        if keycode == 0x39 {
            if down {
                self.caps = !self.caps;
            }
            return;
        }
        if modifier_flag(keycode).is_some() {
            if down {
                self.held.insert(keycode);
            } else {
                self.held.remove(&keycode);
            }
        }
    }

    pub fn flags(&self) -> u64 {
        let held = self.held.iter().filter_map(|&k| modifier_flag(k)).fold(0, |acc, f| acc | f);
        held | if self.caps { FLAG_CAPS } else { 0 }
    }

    pub fn held(&self) -> Vec<u16> {
        self.held.iter().copied().collect()
    }

    /// Caps Lock é estado do Mac, não tecla presa: não é zerado.
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

/// O macOS só reconhece duplo/triplo clique se o evento informar a contagem (clickState).
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(u8, (f64, f64), Instant, i64)>,
}

impl Clicks {
    pub fn press(&mut self, button: u8, at: (f64, f64), now: Instant) -> i64 {
        let count = match self.last {
            Some((b, (x, y), when, n))
                if b == button
                    && now.duration_since(when) < DOUBLE_CLICK
                    && (x - at.0).hypot(y - at.1) < CLICK_SLOP =>
            {
                n + 1
            }
            _ => 1,
        };
        self.last = Some((button, at, now, count));
        count
    }

    pub fn current(&self) -> i64 {
        self.last.map_or(1, |(.., n)| n)
    }
}

/// O teclado físico é ABNT2: com outro layout no Mac, ç, acentos e teclas mortas saem trocados.
pub fn needs_abnt2_warning(input_source_id: &str) -> bool {
    !input_source_id.is_empty() && !input_source_id.contains("ABNT2")
}

/// Converte deltas de roda do Windows (120 por "dente") em pixels de scroll do Mac.
pub fn scroll_pixels(windows_delta: i32) -> i32 {
    // ponytail: 120 → 40 px (≈ 3 linhas); ajustar se a rolagem parecer lenta ou rápida.
    windows_delta / 3
}

/// Origem Quartz (topo à esquerda, y para baixo) → AppKit (base à esquerda, y para cima), com a altura do
/// monitor principal como referência. O canto de baixo de uma tela é `top + height` no Quartz.
pub fn appkit_y(top: f64, height: f64, main_height: f64) -> f64 {
    main_height - top - height
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "macos")]
mod overlay;
#[cfg(target_os = "macos")]
pub use overlay::{show_identify, IdentifyScreen};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartz_vira_appkit_a_partir_do_principal() {
        // Principal 1080 de altura, externo 1440 de altura acima dele (top = -1440): base em 1080 - (-1440) - 1440.
        assert_eq!(appkit_y(0.0, 1080.0, 1080.0), 0.0);
        assert_eq!(appkit_y(-1440.0, 1440.0, 1080.0), 1080.0);
    }

    #[test]
    fn flags_de_modificador() {
        let mut m = Modifiers::default();
        m.key(0x37, true);
        m.key(0x38, true);
        assert_eq!(m.flags(), FLAG_COMMAND | FLAG_SHIFT);
        m.key(0x37, false);
        assert_eq!(m.flags(), FLAG_SHIFT);
        m.key(0x39, true);
        m.key(0x39, false);
        assert_eq!(m.flags(), FLAG_SHIFT | FLAG_CAPS);
        m.key(0x00, true);
        assert_eq!(m.flags(), FLAG_SHIFT | FLAG_CAPS);
    }

    #[test]
    fn release_all_zera_flags() {
        let mut m = Modifiers::default();
        m.key(0x3B, true);
        m.key(0x3A, true);
        assert_eq!(m.held(), vec![0x3A, 0x3B]);
        m.clear();
        assert_eq!(m.flags(), 0);
        assert!(m.held().is_empty());
    }

    #[test]
    fn click_state_conta_cliques_rapidos() {
        let start = Instant::now();
        let mut clicks = Clicks::default();
        assert_eq!(clicks.press(0, (10.0, 10.0), start), 1);
        assert_eq!(clicks.press(0, (11.0, 10.0), start + Duration::from_millis(200)), 2);
        assert_eq!(clicks.press(0, (11.0, 11.0), start + Duration::from_millis(400)), 3);
        assert_eq!(clicks.press(1, (11.0, 11.0), start + Duration::from_millis(500)), 1, "outro botão recomeça");
        assert_eq!(clicks.press(1, (11.0, 11.0), start + Duration::from_millis(1200)), 1, "devagar recomeça");
        assert_eq!(clicks.press(1, (40.0, 11.0), start + Duration::from_millis(1300)), 1, "longe recomeça");
    }

    #[test]
    fn scroll_um_dente() {
        assert_eq!(scroll_pixels(120), 40);
        assert_eq!(scroll_pixels(-120), -40);
    }

    #[test]
    fn setas_levam_flags_de_teclado_numerico_e_fn() {
        for arrow in [0x7B, 0x7C, 0x7D, 0x7E] {
            assert_eq!(key_flags(arrow), FLAG_NUMERIC_PAD | FLAG_FN, "seta {arrow:#x}");
        }
        assert_eq!(key_flags(0x73), FLAG_FN, "Home");
        assert_eq!(key_flags(0x7A), FLAG_FN, "F1");
        assert_eq!(key_flags(0x00), 0, "A");
    }

    #[test]
    fn aviso_de_layout_so_fora_do_abnt2() {
        assert!(!needs_abnt2_warning("com.apple.keylayout.Brazilian-ABNT2"));
        assert!(needs_abnt2_warning("com.apple.keylayout.Brazilian-Pro"));
        assert!(needs_abnt2_warning("com.apple.keylayout.US"));
        assert!(!needs_abnt2_warning(""), "sem informação não avisa");
    }
}
