//! Scancode Windows (set 1) ↔ USB HID usage (página 0x07). Teclado ABNT2 completo.

/// Teclas de mídia da página "consumer" (0x0C), levadas para 0x100 + usage para não colidir com a página 0x07.
pub const MEDIA_PLAY_PAUSE: u16 = 0x1CD;
pub const MEDIA_NEXT: u16 = 0x1B5;
pub const MEDIA_PREVIOUS: u16 = 0x1B6;
pub const MEDIA_STOP: u16 = 0x1B7;

/// Print Screen (HID 0x46): no Mac vira o atalho de captura, não um keycode (ver `print_screen_sequence`).
pub const PRINT_SCREEN: u16 = 0x46;

/// (scancode, extended, HID). NumLock é 0x45 estendido no Windows; Pause chega como 0x45 simples.
// ponytail: busca linear em ~120 entradas; mapa só se aparecer no perfil.
pub(crate) const TABLE: &[(u16, bool, u16)] = &[
    (0x01, false, 0x29),
    (0x02, false, 0x1E),
    (0x03, false, 0x1F),
    (0x04, false, 0x20),
    (0x05, false, 0x21),
    (0x06, false, 0x22),
    (0x07, false, 0x23),
    (0x08, false, 0x24),
    (0x09, false, 0x25),
    (0x0A, false, 0x26),
    (0x0B, false, 0x27),
    (0x0C, false, 0x2D),
    (0x0D, false, 0x2E),
    (0x0E, false, 0x2A),
    (0x0F, false, 0x2B),
    (0x10, false, 0x14),
    (0x11, false, 0x1A),
    (0x12, false, 0x08),
    (0x13, false, 0x15),
    (0x14, false, 0x17),
    (0x15, false, 0x1C),
    (0x16, false, 0x18),
    (0x17, false, 0x0C),
    (0x18, false, 0x12),
    (0x19, false, 0x13),
    (0x1A, false, 0x2F),
    (0x1B, false, 0x30),
    (0x1C, false, 0x28),
    (0x1D, false, 0xE0),
    (0x1E, false, 0x04),
    (0x1F, false, 0x16),
    (0x20, false, 0x07),
    (0x21, false, 0x09),
    (0x22, false, 0x0A),
    (0x23, false, 0x0B),
    (0x24, false, 0x0D),
    (0x25, false, 0x0E),
    (0x26, false, 0x0F),
    (0x27, false, 0x33),
    (0x28, false, 0x34),
    (0x29, false, 0x35),
    (0x2A, false, 0xE1),
    (0x2B, false, 0x31),
    (0x2C, false, 0x1D),
    (0x2D, false, 0x1B),
    (0x2E, false, 0x06),
    (0x2F, false, 0x19),
    (0x30, false, 0x05),
    (0x31, false, 0x11),
    (0x32, false, 0x10),
    (0x33, false, 0x36),
    (0x34, false, 0x37),
    (0x35, false, 0x38),
    (0x36, false, 0xE5),
    // O hook de baixo nível marca o Shift direito como estendido; a injeção usa a linha de cima (primeira com o HID).
    (0x36, true, 0xE5),
    (0x37, false, 0x55),
    (0x38, false, 0xE2),
    (0x39, false, 0x2C),
    (0x3A, false, 0x39),
    (0x3B, false, 0x3A),
    (0x3C, false, 0x3B),
    (0x3D, false, 0x3C),
    (0x3E, false, 0x3D),
    (0x3F, false, 0x3E),
    (0x40, false, 0x3F),
    (0x41, false, 0x40),
    (0x42, false, 0x41),
    (0x43, false, 0x42),
    (0x44, false, 0x43),
    (0x45, false, 0x48),
    (0x46, false, 0x47),
    (0x47, false, 0x5F),
    (0x48, false, 0x60),
    (0x49, false, 0x61),
    (0x4A, false, 0x56),
    (0x4B, false, 0x5C),
    (0x4C, false, 0x5D),
    (0x4D, false, 0x5E),
    (0x4E, false, 0x57),
    (0x4F, false, 0x59),
    (0x50, false, 0x5A),
    (0x51, false, 0x5B),
    (0x52, false, 0x62),
    (0x53, false, 0x63),
    (0x56, false, 0x64),
    (0x57, false, 0x44),
    (0x58, false, 0x45),
    (0x64, false, 0x68),
    (0x65, false, 0x69),
    (0x66, false, 0x6A),
    (0x67, false, 0x6B),
    (0x68, false, 0x6C),
    (0x69, false, 0x6D),
    (0x6A, false, 0x6E),
    (0x6B, false, 0x6F),
    (0x6C, false, 0x70),
    (0x6D, false, 0x71),
    (0x6E, false, 0x72),
    (0x76, false, 0x73),
    (0x70, false, 0x88),
    (0x73, false, 0x87),
    (0x79, false, 0x8A),
    (0x7B, false, 0x8B),
    (0x7D, false, 0x89),
    (0x7E, false, 0x85),
    (0x1C, true, 0x58),
    (0x1D, true, 0xE4),
    (0x35, true, 0x54),
    (0x37, true, 0x46),
    (0x38, true, 0xE6),
    (0x45, true, 0x53),
    (0x47, true, 0x4A),
    (0x48, true, 0x52),
    (0x49, true, 0x4B),
    (0x4B, true, 0x50),
    (0x4D, true, 0x4F),
    (0x4F, true, 0x4D),
    (0x50, true, 0x51),
    (0x51, true, 0x4E),
    (0x52, true, 0x49),
    (0x53, true, 0x4C),
    (0x5B, true, 0xE3),
    (0x5C, true, 0xE7),
    (0x5D, true, 0x65),
    (0x20, true, 0x7F),
    (0x2E, true, 0x81),
    (0x30, true, 0x80),
    (0x22, true, MEDIA_PLAY_PAUSE),
    (0x19, true, MEDIA_NEXT),
    (0x10, true, MEDIA_PREVIOUS),
    (0x24, true, MEDIA_STOP),
];

pub fn win_scancode_to_hid(scancode: u16, extended: bool) -> Option<u16> {
    TABLE.iter().find(|&&(s, e, _)| s == scancode && e == extended).map(|&(_, _, hid)| hid)
}

pub fn hid_to_win_scancode(hid: u16) -> Option<(u16, bool)> {
    TABLE.iter().find(|&&(_, _, h)| h == hid).map(|&(s, e, _)| (s, e))
}

/// HID → virtual keycode do macOS (kVK_*). ScrollLock/Pause viram F13–F15; F21–F24 viram F20. PrintScreen tem tratamento próprio.
/// Teclas físicas: o caractere final depende do layout do Mac (Brasileiro – ABNT2 para bater com o teclado).
const MAC_TABLE: &[(u16, u16)] = &[
    (0x04, 0x00),
    (0x05, 0x0B),
    (0x06, 0x08),
    (0x07, 0x02),
    (0x08, 0x0E),
    (0x09, 0x03),
    (0x0A, 0x05),
    (0x0B, 0x04),
    (0x0C, 0x22),
    (0x0D, 0x26),
    (0x0E, 0x28),
    (0x0F, 0x25),
    (0x10, 0x2E),
    (0x11, 0x2D),
    (0x12, 0x1F),
    (0x13, 0x23),
    (0x14, 0x0C),
    (0x15, 0x0F),
    (0x16, 0x01),
    (0x17, 0x11),
    (0x18, 0x20),
    (0x19, 0x09),
    (0x1A, 0x0D),
    (0x1B, 0x07),
    (0x1C, 0x10),
    (0x1D, 0x06),
    (0x1E, 0x12),
    (0x1F, 0x13),
    (0x20, 0x14),
    (0x21, 0x15),
    (0x22, 0x17),
    (0x23, 0x16),
    (0x24, 0x1A),
    (0x25, 0x1C),
    (0x26, 0x19),
    (0x27, 0x1D),
    (0x28, 0x24),
    (0x29, 0x35),
    (0x2A, 0x33),
    (0x2B, 0x30),
    (0x2C, 0x31),
    (0x2D, 0x1B),
    (0x2E, 0x18),
    (0x2F, 0x21),
    (0x30, 0x1E),
    (0x31, 0x2A),
    (0x32, 0x2A),
    (0x33, 0x29),
    (0x34, 0x27),
    (0x35, 0x32),
    (0x36, 0x2B),
    (0x37, 0x2F),
    (0x38, 0x2C),
    (0x39, 0x39),
    (0x3A, 0x7A),
    (0x3B, 0x78),
    (0x3C, 0x63),
    (0x3D, 0x76),
    (0x3E, 0x60),
    (0x3F, 0x61),
    (0x40, 0x62),
    (0x41, 0x64),
    (0x42, 0x65),
    (0x43, 0x6D),
    (0x44, 0x67),
    (0x45, 0x6F),
    (0x46, 0x69),
    (0x47, 0x6B),
    (0x48, 0x71),
    (0x49, 0x72),
    (0x4A, 0x73),
    (0x4B, 0x74),
    (0x4C, 0x75),
    (0x4D, 0x77),
    (0x4E, 0x79),
    (0x4F, 0x7C),
    (0x50, 0x7B),
    (0x51, 0x7D),
    (0x52, 0x7E),
    (0x53, 0x47),
    (0x54, 0x4B),
    (0x55, 0x43),
    (0x56, 0x4E),
    (0x57, 0x45),
    (0x58, 0x4C),
    (0x59, 0x53),
    (0x5A, 0x54),
    (0x5B, 0x55),
    (0x5C, 0x56),
    (0x5D, 0x57),
    (0x5E, 0x58),
    (0x5F, 0x59),
    (0x60, 0x5B),
    (0x61, 0x5C),
    (0x62, 0x52),
    (0x63, 0x41),
    (0x64, 0x0A),
    (0x67, 0x51),
    (0x68, 0x69),
    (0x69, 0x6B),
    (0x6A, 0x71),
    (0x6B, 0x6A),
    (0x6C, 0x40),
    (0x6D, 0x4F),
    (0x6E, 0x50),
    (0x6F, 0x5A),
    (0x70, 0x5A),
    (0x71, 0x5A),
    (0x72, 0x5A),
    (0x73, 0x5A),
    (0x85, 0x5F),
    (0x87, 0x5E),
    (0x88, 0x68),
    (0x89, 0x5D),
    (0xE0, 0x3B),
    (0xE1, 0x38),
    (0xE2, 0x3A),
    (0xE3, 0x37),
    (0xE4, 0x3E),
    (0xE5, 0x3C),
    (0xE6, 0x3D),
    (0xE7, 0x36),
];

pub fn hid_to_mac(hid: u16) -> Option<u16> {
    MAC_TABLE.iter().find(|&&(h, _)| h == hid).map(|&(_, mac)| mac)
}

/// kVK_* do Mac → HID, inverso de `hid_to_mac`. Keycode dividido por mais de uma tecla (F20–F24) volta à primeira.
pub fn mac_to_hid(keycode: u16) -> Option<u16> {
    MAC_TABLE.iter().find(|&&(_, mac)| mac == keycode).map(|&(hid, _)| hid)
}

/// Tecla física do Mac → scancode do Windows, como o servidor a envia. Com `remap`, Cmd e Ctrl trocam de lugar
/// (a mesma troca que o cliente Mac faz ao receber).
pub fn mac_key_to_win(keycode: u16, remap: bool) -> Option<(u16, bool)> {
    hid_to_win_scancode(remap_for_mac(mac_to_hid(keycode)?, remap))
}

/// Inverso de `mac_key_to_win`: scancode do Windows → kVK_* do Mac, para soltar no Mac a tecla apertada aqui.
pub fn win_to_mac_key(scancode: u16, extended: bool, remap: bool) -> Option<u16> {
    hid_to_mac(remap_for_mac(win_scancode_to_hid(scancode, extended)?, remap))
}

/// Mídia e volume no Mac: NX_KEYTYPE_* (evento de sistema; o keycode de volume não muda o volume).
pub fn hid_to_mac_media(hid: u16) -> Option<u32> {
    Some(match hid {
        0x80 => 0,
        0x81 => 1,
        0x7F => 7,
        MEDIA_PLAY_PAUSE => 16,
        MEDIA_NEXT => 17,
        MEDIA_PREVIOUS => 18,
        _ => return None,
    })
}

/// Teclado de Windows no Mac: Ctrl↔Cmd (Win vira Ctrl). Alt já é Option; AltGr (Alt direito) é Option direito.
pub fn remap_for_mac(hid: u16, enabled: bool) -> u16 {
    match (enabled, hid) {
        (true, 0xE0) => 0xE3,
        (true, 0xE3) => 0xE0,
        (true, 0xE4) => 0xE7,
        (true, 0xE7) => 0xE4,
        _ => hid,
    }
}

/// HID 0x00–0x94 → código evdev do Linux (KEY_*), a tabela `hid_keyboard` do kernel; 0 = sem tecla.
/// O X11 usa o mesmo código + 8. Teclas físicas, como no Mac: o caractere depende do layout escolhido.
#[rustfmt::skip]
const EVDEV_TABLE: [u8; 0x95] = [
      0,   0,   0,   0,  30,  48,  46,  32,  18,  33,  34,  35,  23,  36,  37,  38,
     50,  49,  24,  25,  16,  19,  31,  20,  22,  47,  17,  45,  21,  44,   2,   3,
      4,   5,   6,   7,   8,   9,  10,  11,  28,   1,  14,  15,  57,  12,  13,  26,
     27,  43,  43,  39,  40,  41,  51,  52,  53,  58,  59,  60,  61,  62,  63,  64,
     65,  66,  67,  68,  87,  88,  99,  70, 119, 110, 102, 104, 111, 107, 109, 106,
    105, 108, 103,  69,  98,  55,  74,  78,  96,  79,  80,  81,  75,  76,  77,  71,
     72,  73,  82,  83,  86, 127, 116, 117, 183, 184, 185, 186, 187, 188, 189, 190,
    191, 192, 193, 194, 134, 138, 130, 132, 128, 129, 131, 137, 133, 135, 136, 113,
    115, 114,   0,   0,   0, 121,   0,  89,  93, 124,  92,  94,  95,   0,   0,   0,
    122, 123,  90,  91,  85,
];

pub fn hid_to_evdev(hid: u16) -> Option<u16> {
    let code = match hid {
        0xE0..=0xE7 => [29, 42, 56, 125, 97, 54, 100, 126][(hid - 0xE0) as usize],
        MEDIA_PLAY_PAUSE => 164,
        MEDIA_NEXT => 163,
        MEDIA_PREVIOUS => 165,
        MEDIA_STOP => 166,
        _ => *EVDEV_TABLE.get(hid as usize)? as u16,
    };
    (code != 0).then_some(code)
}

/// evdev (KEY_*) → HID. Inverse of `hid_to_evdev` for keys the Linux server captures.
pub fn evdev_to_hid(code: u16) -> Option<u16> {
    const MODIFIERS: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];
    if let Some(index) = MODIFIERS.iter().position(|modifier| *modifier == code) {
        return Some(0xE0 + index as u16);
    }
    match code {
        164 => Some(MEDIA_PLAY_PAUSE),
        163 => Some(MEDIA_NEXT),
        165 => Some(MEDIA_PREVIOUS),
        166 => Some(MEDIA_STOP),
        _ => EVDEV_TABLE.iter().position(|&entry| entry != 0 && u16::from(entry) == code).map(|hid| hid as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_do_mac_vira_ctrl_e_option_vira_alt() {
        assert_eq!(mac_key_to_win(0x37, true), Some((0x1D, false)), "Cmd físico é Ctrl do Windows");
        assert_eq!(mac_key_to_win(0x37, false), Some((0x5B, true)), "sem troca, Cmd é a tecla Windows");
        assert_eq!(mac_key_to_win(0x3B, true), Some((0x5B, true)), "Ctrl físico é a tecla Windows");
        assert_eq!(mac_key_to_win(0x3A, true), Some((0x38, false)), "Option é Alt");
    }

    #[test]
    fn f14_do_mac_e_scroll_lock_e_home_e_numero_viram_atalhos() {
        assert_eq!(mac_key_to_win(0x6B, false), Some((0x46, false)), "F14 = Scroll Lock (prende o cursor)");
        assert_eq!(mac_key_to_win(0x73, false), Some((0x47, true)), "Home (emergência)");
        assert_eq!(mac_key_to_win(0x12, false), Some((0x02, false)), "1 (salto)");
    }

    #[test]
    fn tecla_do_mac_volta_ao_mesmo_keycode() {
        for keycode in [0x37, 0x36, 0x3B, 0x3A, 0x6B, 0x73, 0x12, 0x00] {
            let (scancode, extended) = mac_key_to_win(keycode, true).unwrap();
            assert_eq!(win_to_mac_key(scancode, extended, true), Some(keycode), "keycode {keycode:#04x}");
        }
    }

    #[test]
    fn abnt2_teclas_exclusivas() {
        assert_eq!(win_scancode_to_hid(0x56, false), Some(0x64));
        assert_eq!(win_scancode_to_hid(0x73, false), Some(0x87));
        assert_eq!(win_scancode_to_hid(0x7E, false), Some(0x85));
    }

    #[test]
    fn shift_direito_vem_marcado_como_estendido_no_hook() {
        assert_eq!(win_scancode_to_hid(0x36, true), Some(0xE5));
        assert_eq!(hid_to_win_scancode(0xE5), Some((0x36, false)), "injeção usa o scancode normal");
    }

    #[test]
    fn extendidas() {
        assert_eq!(win_scancode_to_hid(0x1D, true), Some(0xE4));
        assert_eq!(win_scancode_to_hid(0x48, true), Some(0x52));
        assert_eq!(win_scancode_to_hid(0x5B, true), Some(0xE3));
        assert_eq!(win_scancode_to_hid(0x1D, false), Some(0xE0));
    }

    #[test]
    fn roundtrip_tabela_completa() {
        for &(scancode, extended, hid) in TABLE {
            assert_eq!(win_scancode_to_hid(scancode, extended), Some(hid), "scancode {scancode:#x}");
            // Apelidos (Shift direito estendido) injetam pela linha principal do mesmo HID.
            assert_eq!(
                hid_to_win_scancode(hid).and_then(|(s, e)| win_scancode_to_hid(s, e)),
                Some(hid),
                "hid {hid:#x}"
            );
        }
        assert_eq!(win_scancode_to_hid(0x7F, true), None);
    }

    #[test]
    fn teclas_de_midia_do_windows() {
        assert_eq!(win_scancode_to_hid(0x22, true), Some(MEDIA_PLAY_PAUSE));
        assert_eq!(win_scancode_to_hid(0x19, true), Some(MEDIA_NEXT));
        assert_eq!(win_scancode_to_hid(0x10, true), Some(MEDIA_PREVIOUS));
        assert_eq!(win_scancode_to_hid(0x24, true), Some(MEDIA_STOP));
        assert_eq!(hid_to_win_scancode(MEDIA_PLAY_PAUSE), Some((0x22, true)));
    }

    #[test]
    fn midia_no_mac_vira_tecla_de_sistema() {
        // NX_KEYTYPE_*: volume e mudo também, porque keycode de volume não muda o volume no macOS.
        assert_eq!(hid_to_mac_media(0x80), Some(0));
        assert_eq!(hid_to_mac_media(0x81), Some(1));
        assert_eq!(hid_to_mac_media(0x7F), Some(7));
        assert_eq!(hid_to_mac_media(MEDIA_PLAY_PAUSE), Some(16));
        assert_eq!(hid_to_mac_media(MEDIA_NEXT), Some(17));
        assert_eq!(hid_to_mac_media(MEDIA_PREVIOUS), Some(18));
        assert_eq!(hid_to_mac_media(0x04), None);
        assert_eq!(hid_to_mac(0x80), None, "volume não vai como keycode");
    }

    #[test]
    fn hid_para_mac_abnt2() {
        assert_eq!(hid_to_mac(0x64), Some(0x0A));
        assert_eq!(hid_to_mac(0x87), Some(0x5E));
        assert_eq!(hid_to_mac(0x85), Some(0x5F));
    }

    #[test]
    fn letras_e_setas() {
        assert_eq!(hid_to_mac(0x04), Some(0x00));
        assert_eq!(hid_to_mac(0x1D), Some(0x06));
        assert_eq!(hid_to_mac(0x52), Some(0x7E));
        assert_eq!(hid_to_mac(0xE3), Some(0x37));
        assert_eq!(hid_to_mac(0x02), None);
    }

    #[test]
    fn todo_hid_do_windows_tem_destino_no_mac_exceto_menu_e_ime() {
        for &(_, _, hid) in TABLE {
            if matches!(hid, 0x65 | 0x8A | 0x8B | MEDIA_STOP) {
                continue;
            }
            assert!(hid_to_mac(hid).or(hid_to_mac_media(hid).map(|_| 0)).is_some(), "hid {hid:#x} sem destino no Mac");
        }
    }

    #[test]
    fn evdev_do_linux_volta_para_o_hid() {
        assert_eq!(evdev_to_hid(30), Some(0x04), "KEY_A");
        assert_eq!(evdev_to_hid(29), Some(0xE0), "Ctrl esquerdo");
        assert_eq!(evdev_to_hid(164), Some(MEDIA_PLAY_PAUSE));
        for hid in 0x04..0x94 {
            if let Some(code) = hid_to_evdev(hid) {
                let back = evdev_to_hid(code);
                assert!(back.is_some(), "evdev {code} do hid {hid:#x} não volta");
            }
        }
    }

    #[test]
    fn hid_para_evdev_do_linux() {
        assert_eq!(hid_to_evdev(0x04), Some(30), "A");
        assert_eq!(hid_to_evdev(0x28), Some(28), "Enter");
        assert_eq!(hid_to_evdev(0x52), Some(103), "seta para cima");
        assert_eq!(hid_to_evdev(0x45), Some(88), "F12");
        assert_eq!(hid_to_evdev(0xE3), Some(125), "Win vira Super");
        assert_eq!(hid_to_evdev(0xE6), Some(100), "AltGr");
        assert_eq!(hid_to_evdev(0x80), Some(115), "volume para cima");
        assert_eq!(hid_to_evdev(MEDIA_PLAY_PAUSE), Some(164));
        assert_eq!(hid_to_evdev(0x02), None);
    }

    #[test]
    fn abnt2_no_linux() {
        assert_eq!(hid_to_evdev(0x64), Some(86), "\\ | ao lado do Shift esquerdo");
        assert_eq!(hid_to_evdev(0x87), Some(89), "/ ? (KEY_RO)");
        assert_eq!(hid_to_evdev(0x85), Some(121), "ponto do teclado numérico (KEY_KPCOMMA)");
    }

    #[test]
    fn todo_hid_do_windows_tem_destino_no_linux() {
        for &(_, _, hid) in TABLE {
            assert!(hid_to_evdev(hid).is_some(), "hid {hid:#x} sem destino no Linux");
        }
    }

    #[test]
    fn print_screen_vai_como_hid_46_estendido() {
        assert_eq!(win_scancode_to_hid(0x37, true), Some(PRINT_SCREEN));
        assert_eq!(
            hid_to_win_scancode(PRINT_SCREEN),
            Some((0x37, true)),
            "injeção no Windows usa PrintScreen estendido"
        );
        assert_eq!(hid_to_evdev(PRINT_SCREEN), Some(99), "KEY_SYSRQ no Linux");
        assert_eq!(win_scancode_to_hid(0x37, false), Some(0x55), "asterisco do teclado numérico não é PrintScreen");
    }

    #[test]
    fn remap_ctrl_vira_cmd() {
        assert_eq!(remap_for_mac(0xE0, true), 0xE3);
        assert_eq!(remap_for_mac(0xE3, true), 0xE0);
        assert_eq!(remap_for_mac(0xE4, true), 0xE7);
        assert_eq!(remap_for_mac(0xE7, true), 0xE4);
        assert_eq!(remap_for_mac(0xE0, false), 0xE0);
        assert_eq!(remap_for_mac(0xE6, true), 0xE6);
        assert_eq!(remap_for_mac(0x04, true), 0x04);
    }
}
