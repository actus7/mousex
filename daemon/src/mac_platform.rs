//! Plataforma macOS (só cliente): injeção via CGEvent, monitores em pontos e clipboard NSPasteboard.
use crate::{
    client::{Platform, numbered},
    clip::Clip,
    config::{Config, data_dir},
};
use mousex_core::{keymap, proto::MonitorInfo};
use mousex_platform_macos as mac;

/// Monitores em pontos, com escala 1.0: o macOS posiciona as telas em pontos, e escalas diferentes por tela
/// (Retina 2.0 ao lado de externo 1.0) sobreporiam as telas no layout. O deslocamento do cursor é o mesmo.
pub fn local_monitors() -> Vec<MonitorInfo> {
    mac::displays()
        .into_iter()
        .map(|d| MonitorInfo {
            id: d.id.to_string(),
            x: d.x.round() as i32,
            y: d.y.round() as i32,
            width: d.width.round() as i32,
            height: d.height.round() as i32,
            scale: 1.0,
        })
        .collect()
}

pub struct MacPlatform {
    injector: mac::Injector,
    origins: Vec<(f64, f64)>,
    /// Ctrl↔Cmd (teclado de Windows usado como teclado de Mac).
    remap: bool,
}

impl MacPlatform {
    pub fn new(monitors: &[MonitorInfo]) -> Self {
        Self {
            injector: mac::Injector::default(),
            origins: monitors.iter().map(|m| (m.x as f64, m.y as f64)).collect(),
            remap: Config::load(&data_dir()).is_none_or(|c| c.remap_modifiers),
        }
    }
}

pub type Native = MacPlatform;

// Falhas de injeção (permissão de Acessibilidade ausente) são ignoradas: nada a fazer além de não travar.
impl Platform for MacPlatform {
    fn key(&mut self, hid: u16, down: bool) {
        // Print Screen dispara a captura no keydown; o keyup não tem o que fazer.
        if hid == keymap::PRINT_SCREEN {
            if down {
                let _ = self.injector.print_screen();
            }
            return;
        }
        if let Some(media) = keymap::hid_to_mac_media(hid) {
            let _ = mac::media_key(media, down);
            return;
        }
        if let Some(keycode) = keymap::hid_to_mac(keymap::remap_for_mac(hid, self.remap)) {
            let _ = self.injector.key(keycode, down);
        }
    }

    fn move_to(&mut self, monitor: usize, x: i32, y: i32) {
        if let Some(&(ox, oy)) = self.origins.get(monitor) {
            let _ = self.injector.move_to(ox + x as f64, oy + y as f64);
        }
    }

    fn button(&mut self, button: u8, down: bool) {
        let _ = self.injector.button(button, down);
    }

    fn wheel(&mut self, dx: i32, dy: i32) {
        let _ = self.injector.scroll(dx, dy);
    }

    fn release_all(&mut self) {
        self.injector.release_all();
    }

    fn clipboard_seq(&self) -> u32 {
        clipboard_seq()
    }

    fn clipboard_read(&self) -> Option<Clip> {
        clipboard_read()
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        local_monitors()
    }

    fn set_monitors(&mut self, monitors: &[MonitorInfo]) {
        self.origins = monitors.iter().map(|m| (m.x as f64, m.y as f64)).collect();
    }

    fn keep_awake(&mut self) {
        if !mac::declare_user_activity() {
            crate::log("macOS refused to keep the display awake");
        }
    }

    /// Ctrl+Cmd+Q: o atalho de bloqueio do próprio macOS.
    fn identify(&mut self, screens: &[(String, u32)]) {
        show_identify(screens);
    }

    fn lock_screen(&mut self) {
        const CONTROL: u16 = 0x3B;
        const COMMAND: u16 = 0x37;
        const Q: u16 = 0x0C;
        for (keycode, down) in
            [(CONTROL, true), (COMMAND, true), (Q, true), (Q, false), (COMMAND, false), (CONTROL, false)]
        {
            let _ = self.injector.key(keycode, down);
        }
    }

    fn clipboard_write(&mut self, clip: Clip) {
        clipboard_write(&clip);
    }
}

pub fn clipboard_seq() -> u32 {
    mac::clipboard_seq() as u32
}

pub fn clipboard_read() -> Option<Clip> {
    match mac::clipboard_read()? {
        mac::Clip::Text(text) => Some(Clip::Text(text)),
        mac::Clip::Png(png) => Some(Clip::Png(png)),
        mac::Clip::Files(paths) => Some(Clip::Files(paths)),
    }
}

pub fn clipboard_write(clip: &Clip) {
    let native = match clip {
        Clip::Text(text) => mac::Clip::Text(text.clone()),
        Clip::Png(png) => mac::Clip::Png(png.clone()),
        Clip::Files(paths) => mac::Clip::Files(paths.clone()),
    };
    if let Err(error) = mac::clipboard_write(&native) {
        crate::log(&format!("failed to write the clipboard: {error}"));
    }
}

/// Números nas telas (Identificar telas do editor). Ctrl+Cmd+Q é o atalho de bloqueio do próprio macOS.
pub fn show_identify(screens: &[(String, u32)]) {
    let found: Vec<mac::IdentifyScreen> = numbered(&local_monitors(), screens)
        .into_iter()
        .map(|(m, number)| mac::IdentifyScreen { x: m.x, y: m.y, width: m.width, height: m.height, number })
        .collect();
    mac::show_identify(&found);
}

/// Painel de transferência no Mac. ponytail: sem botão Cancelar (exige classe ObjC com ação); a ação é descartada.
pub fn show_panel(
    title: &str,
    poll: Box<dyn Fn() -> Option<(String, Option<u8>)> + Send>,
    cancel: Option<(String, Box<dyn Fn() + Send>)>,
) {
    drop(cancel);
    mac::show_panel(title, poll);
}

/// No Mac o agente já roda como o usuário.
pub fn as_standard_user<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    Ok(f())
}

/// Pasta de recebidos dentro da pasta de dados do usuário.
pub fn ensure_inbox(inbox: &std::path::Path, _check_owner: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(inbox)
}

// ponytail: sem checagem de espaço no Mac (só o limite de 2 GB); statvfs se o disco do Mac encher.
pub fn free_space(_path: &std::path::Path) -> Option<u64> {
    None
}

pub fn ask_yes_no(title: &str, text: &str) -> bool {
    mac::ask_yes_no(title, text)
}

/// Pede a permissão de Acessibilidade (o macOS mostra o aviso na primeira vez) e avisa, uma vez só,
/// se o layout de teclado não for ABNT2.
pub fn prepare() -> bool {
    warn_keyboard_layout();
    mac::ensure_accessibility(true)
}

fn warn_keyboard_layout() {
    let marker = data_dir().join("aviso-abnt2");
    let Some(id) = mac::input_source_id() else { return };
    // ponytail: o servidor não informa o layout do teclado dele; o aviso supõe ABNT2 só em sistemas em português.
    // Para outros teclados, mandar o layout do servidor no Hello e comparar.
    let portuguese = mousex_core::i18n::lang() == mousex_core::i18n::Lang::Pt;
    if !portuguese || !mac::needs_abnt2_warning(&id) || marker.exists() {
        return;
    }
    crate::log(&format!("Mac keyboard layout is {id}, not ABNT2"));
    mac::notify(
        "MouseX",
        "O teclado do computador principal é ABNT2. Para ç e acentos saírem certos neste Mac, escolha \"Brasileiro – ABNT2\" em Ajustes do Sistema > Teclado > Fontes de Entrada.",
    );
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(marker, id);
}
