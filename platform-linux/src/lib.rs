//! Backend Linux (cliente). Injeção por XTest no X11 ou pelo portal RemoteDesktop + libei no Wayland;
//! telas pelo RandR e clipboard pelo X11 (no Wayland, o XWayland repassa os dois). Teclas são códigos evdev.
//! A lógica pura (botões, roda, regiões) fica aqui e é testada em qualquer SO.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

/// Um dente de roda do Windows.
pub const WHEEL_DELTA: i32 = 120;

/// Botão do protocolo (0 esquerdo, 1 direito, 2 meio, 3 voltar, 4 avançar) → botão do X11.
pub fn x11_button(button: u8) -> Option<u8> {
    [1, 3, 2, 8, 9].get(button as usize).copied()
}

/// Botão do protocolo → código evdev (BTN_LEFT…BTN_EXTRA), usado pelo libei.
pub fn evdev_button(button: u8) -> Option<u32> {
    (button < 5).then_some(0x110 + button as u32)
}

/// Junta deltas de roda (touchpad manda frações de dente) e devolve dentes inteiros; positivo = cima/direita.
#[derive(Default)]
pub struct Wheel {
    x: i32,
    y: i32,
}

impl Wheel {
    pub fn notches(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        self.x += dx;
        self.y += dy;
        let (nx, ny) = (self.x / WHEEL_DELTA, self.y / WHEEL_DELTA);
        self.x -= nx * WHEEL_DELTA;
        self.y -= ny * WHEEL_DELTA;
        (nx, ny)
    }
}

/// Retângulo de tela em pixels lógicos globais.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Ponto mais próximo dentro de alguma região: o libei descarta posições fora das telas dele (as telas
/// vistas pelo XWayland podem diferir das do compositor por arredondamento de escala).
pub fn clamp_to_regions(x: i32, y: i32, regions: &[Rect]) -> Option<(i32, i32)> {
    regions
        .iter()
        .filter(|r| r.width > 0 && r.height > 0)
        .map(|r| (x.clamp(r.x, r.x + r.width - 1), y.clamp(r.y, r.y + r.height - 1)))
        .min_by_key(|&(cx, cy)| (i64::from(cx - x)).pow(2) + (i64::from(cy - y)).pow(2))
}

/// Ponto global das telas do X11 (`screens`) → ponto nas regiões do libei. As duas visões podem ter
/// escalas diferentes (pixels físicos × lógicos), então a conversão é proporcional dentro da tela, e as
/// telas casam pela posição. Listas de tamanhos diferentes caem no ponto mais próximo.
pub fn map_to_regions(x: i32, y: i32, screens: &[Rect], regions: &[Rect]) -> Option<(i32, i32)> {
    let contains = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
    let by_position = |list: &[Rect]| {
        let mut sorted = list.to_vec();
        sorted.sort_by_key(|r| (r.x, r.y));
        sorted
    };
    let (screens, sorted_regions) = (by_position(screens), by_position(regions));
    let mapped = screens.iter().position(contains).filter(|_| screens.len() == sorted_regions.len()).map(|i| {
        let (s, r) = (screens[i], sorted_regions[i]);
        let scale = |offset: i32, from: i32, to: i32| (i64::from(offset) * i64::from(to) / i64::from(from)) as i32;
        (r.x + scale(x - s.x, s.width, r.width), r.y + scale(y - s.y, s.height, r.height))
    });
    let (x, y) = mapped.unwrap_or((x, y));
    clamp_to_regions(x, y, regions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn botoes_do_protocolo_no_x11_e_no_evdev() {
        assert_eq!([0, 1, 2, 3, 4].map(x11_button), [Some(1), Some(3), Some(2), Some(8), Some(9)]);
        assert_eq!(
            [0, 1, 2, 3, 4].map(evdev_button),
            [Some(0x110), Some(0x111), Some(0x112), Some(0x113), Some(0x114)]
        );
        assert_eq!(x11_button(5), None);
        assert_eq!(evdev_button(5), None);
    }

    #[test]
    fn roda_de_mouse_vira_um_dente_por_120() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.notches(0, 240), (0, 2));
        assert_eq!(wheel.notches(-120, -120), (-1, -1));
    }

    #[test]
    fn touchpad_junta_fracoes_e_guarda_o_resto() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.notches(0, 50), (0, 0));
        assert_eq!(wheel.notches(0, 50), (0, 0));
        assert_eq!(wheel.notches(0, 50), (0, 1), "150 = um dente e sobra 30");
        assert_eq!(wheel.notches(0, -150), (0, -1), "30 - 150 = -120");
        assert_eq!(wheel.notches(0, 0), (0, 0));
    }

    #[test]
    fn ponto_dentro_de_uma_tela_fica_igual() {
        let screens =
            [Rect { x: 0, y: 0, width: 1920, height: 1080 }, Rect { x: 1920, y: 0, width: 1280, height: 1024 }];
        assert_eq!(clamp_to_regions(2000, 500, &screens), Some((2000, 500)));
        assert_eq!(clamp_to_regions(0, 0, &screens), Some((0, 0)));
    }

    #[test]
    fn mesma_escala_nas_duas_visoes_nao_muda_o_ponto() {
        let screens =
            [Rect { x: 0, y: 0, width: 1920, height: 1080 }, Rect { x: 1920, y: 0, width: 1280, height: 1024 }];
        assert_eq!(map_to_regions(2000, 500, &screens, &screens), Some((2000, 500)));
    }

    #[test]
    fn tela_em_pixels_fisicos_vira_regiao_em_pixels_logicos() {
        // XWayland em pixels físicos (HiDPI 2x), libei em lógicos: proporcional, tela por tela.
        let screens =
            [Rect { x: 0, y: 0, width: 3840, height: 2160 }, Rect { x: 3840, y: 0, width: 1920, height: 1080 }];
        let regions =
            [Rect { x: 0, y: 0, width: 1920, height: 1080 }, Rect { x: 1920, y: 0, width: 1920, height: 1080 }];
        assert_eq!(map_to_regions(3000, 1000, &screens, &regions), Some((1500, 500)));
        assert_eq!(map_to_regions(4000, 100, &screens, &regions), Some((2080, 100)));
    }

    #[test]
    fn telas_casam_pela_posicao_nao_pela_ordem_da_lista() {
        let screens =
            [Rect { x: 1920, y: 0, width: 1920, height: 1080 }, Rect { x: 0, y: 0, width: 1920, height: 1080 }];
        let regions = [Rect { x: 0, y: 0, width: 960, height: 540 }, Rect { x: 960, y: 0, width: 960, height: 540 }];
        assert_eq!(map_to_regions(100, 100, &screens, &regions), Some((50, 50)));
    }

    #[test]
    fn quantidade_diferente_de_telas_cai_no_mais_proximo() {
        let screens = [Rect { x: 0, y: 0, width: 1920, height: 1080 }];
        let regions =
            [Rect { x: 0, y: 0, width: 1000, height: 1000 }, Rect { x: 1000, y: 0, width: 1000, height: 1000 }];
        assert_eq!(map_to_regions(1500, 1050, &screens, &regions), Some((1500, 999)));
    }

    #[test]
    fn ponto_fora_vai_para_a_borda_da_tela_mais_proxima() {
        let screens =
            [Rect { x: 0, y: 0, width: 1920, height: 1080 }, Rect { x: 1920, y: 0, width: 1280, height: 1024 }];
        assert_eq!(clamp_to_regions(2500, 1050, &screens), Some((2500, 1023)), "abaixo da tela menor");
        assert_eq!(clamp_to_regions(1920, 1080, &screens), Some((1919, 1079)), "logo após o canto da primeira");
        assert_eq!(clamp_to_regions(-5, -5, &screens), Some((0, 0)));
        assert_eq!(clamp_to_regions(10, 10, &[]), None);
    }
}
