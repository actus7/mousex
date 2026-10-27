//! Plataforma Linux (só cliente): injeção por XTest (X11) ou pelo portal RemoteDesktop (Wayland),
//! telas pelo RandR e clipboard pelo X11.
use crate::{client::Platform, clip::Clip, config::data_dir};
use mousex_core::{MAX_CLIPBOARD_TEXT_BYTES, keymap, proto::MonitorInfo};
use mousex_platform_linux as linux;

/// Telas em pixels, com escala 1.0, como o X11 (ou o XWayland) as posiciona.
pub fn local_monitors() -> Vec<MonitorInfo> {
    linux::displays()
        .into_iter()
        .map(|d| MonitorInfo {
            id: d.id,
            x: d.rect.x,
            y: d.rect.y,
            width: d.rect.width,
            height: d.rect.height,
            scale: 1.0,
        })
        .collect()
}

/// Wayland pelo portal; sem portal com libei (WSLg, compositor antigo), XTest, que no Wayland só
/// alcança as janelas do XWayland.
fn injector() -> Option<linux::Injector> {
    if linux::wayland_session() {
        match linux::Injector::portal(data_dir().join("portal-token")) {
            Ok(injector) => return Some(injector),
            Err(error) => crate::log(&format!("Wayland remote control unavailable ({error}); falling back to X11")),
        }
    }
    linux::Injector::x11().inspect_err(|error| crate::log(&format!("X11 input injection unavailable: {error}"))).ok()
}

pub struct LinuxPlatform {
    injector: Option<linux::Injector>,
    clipboard: Option<linux::Clipboard>,
    origins: Vec<(i32, i32)>,
    denied_logged: bool,
}

impl LinuxPlatform {
    pub fn new(monitors: &[MonitorInfo]) -> Self {
        let clipboard =
            linux::Clipboard::new().inspect_err(|error| crate::log(&format!("clipboard unavailable: {error}"))).ok();
        let mut platform = Self { injector: injector(), clipboard, origins: Vec::new(), denied_logged: false };
        platform.set_monitors(monitors);
        platform
    }

    /// A sessão do portal acabou depois de funcionar (compositor reiniciado): sair para o systemd religar
    /// o agente, que reabre a sessão pelo token. Permissão negada fica no log uma vez, sem repetir a
    /// pergunta. Outros erros de injeção são ignorados, como no Mac.
    fn check(&mut self, result: std::io::Result<()>) {
        let Err(error) = result else { return };
        match error.kind() {
            std::io::ErrorKind::BrokenPipe => {
                crate::log(&format!("remote control ended: {error}; restarting"));
                std::process::exit(1);
            }
            std::io::ErrorKind::PermissionDenied if !self.denied_logged => {
                self.denied_logged = true;
                crate::log(&format!("remote control not allowed: {error}; input from the main computer is ignored"));
            }
            _ => {}
        }
    }
}

pub type Native = LinuxPlatform;

impl Platform for LinuxPlatform {
    fn key(&mut self, hid: u16, down: bool) {
        crate::activity::input(if down { "key:down" } else { "key:up" });
        if let (Some(injector), Some(code)) = (self.injector.as_mut(), keymap::hid_to_evdev(hid)) {
            let result = injector.key(code, down);
            self.check(result);
        }
    }

    fn move_to(&mut self, monitor: usize, x: i32, y: i32) {
        crate::activity::pointer(&format!("{monitor}:{x}:{y}"));
        if let (Some(injector), Some(&(ox, oy))) = (self.injector.as_mut(), self.origins.get(monitor)) {
            let result = injector.move_to(ox + x, oy + y);
            self.check(result);
        }
    }

    fn button(&mut self, button: u8, down: bool) {
        crate::activity::input(&format!("button:{button}:{}", if down { "down" } else { "up" }));
        if let Some(injector) = self.injector.as_mut() {
            let result = injector.button(button, down);
            self.check(result);
        }
    }

    fn wheel(&mut self, dx: i32, dy: i32) {
        crate::activity::input("wheel");
        if let Some(injector) = self.injector.as_mut() {
            let result = injector.scroll(dx, dy);
            self.check(result);
        }
    }

    fn release_all(&mut self) {
        crate::activity::input("release");
        if let Some(injector) = self.injector.as_mut() {
            injector.release_all();
        }
    }

    fn clipboard_seq(&self) -> u32 {
        linux::clipboard_seq()
    }

    fn clipboard_read(&self) -> Option<Clip> {
        match self.clipboard.as_ref()?.read()? {
            linux::Clip::Text(text) => (text.len() <= MAX_CLIPBOARD_TEXT_BYTES).then_some(Clip::Text(text)),
            linux::Clip::Png(png) => (png.len() <= crate::clip::MAX_CLIPBOARD_IMAGE_BYTES).then_some(Clip::Png(png)),
            linux::Clip::Files(paths) => Some(Clip::Files(paths)),
        }
    }

    fn clipboard_write(&mut self, clip: Clip) {
        let native = match clip {
            Clip::Text(text) => linux::Clip::Text(text),
            Clip::Png(png) => linux::Clip::Png(png),
            Clip::Files(paths) => linux::Clip::Files(paths),
        };
        if let Some(Err(error)) = self.clipboard.as_ref().map(|clipboard| clipboard.write(&native)) {
            crate::log(&format!("clipboard write failed: {error}"));
        }
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        local_monitors()
    }

    fn set_monitors(&mut self, monitors: &[MonitorInfo]) {
        self.origins = monitors.iter().map(|m| (m.x, m.y)).collect();
        let screens: Vec<linux::Rect> =
            monitors.iter().map(|m| linux::Rect { x: m.x, y: m.y, width: m.width, height: m.height }).collect();
        if let Some(injector) = self.injector.as_mut() {
            injector.set_screens(&screens);
        }
    }

    fn keep_awake(&mut self) {
        linux::keep_awake();
    }

    fn lock_screen(&mut self) {
        if let Err(error) = linux::lock_screen() {
            crate::log(&format!("could not lock the screen: {error}"));
        }
    }
}

/// No Linux o agente já roda como o usuário.
pub fn as_standard_user<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    Ok(f())
}

/// Pasta de recebidos dentro da pasta de dados do usuário.
pub fn ensure_inbox(inbox: &std::path::Path, _check_owner: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(inbox)
}

// ponytail: sem checagem de espaço (só o limite de 2 GB), como no Mac; statvfs se um disco encher.
pub fn free_space(_path: &std::path::Path) -> Option<u64> {
    None
}

fn shared_clipboard() -> Option<&'static linux::Clipboard> {
    static CLIP: std::sync::OnceLock<Option<linux::Clipboard>> = std::sync::OnceLock::new();
    CLIP.get_or_init(|| {
        linux::Clipboard::new().inspect_err(|error| crate::log(&format!("clipboard unavailable: {error}"))).ok()
    })
    .as_ref()
}

pub fn clipboard_seq() -> u32 {
    linux::clipboard_seq()
}

pub fn clipboard_read() -> Option<crate::clip::Clip> {
    match shared_clipboard()?.read()? {
        linux::Clip::Text(text) => {
            (text.len() <= mousex_core::MAX_CLIPBOARD_TEXT_BYTES).then_some(crate::clip::Clip::Text(text))
        }
        linux::Clip::Png(png) => {
            (png.len() <= crate::clip::MAX_CLIPBOARD_IMAGE_BYTES).then_some(crate::clip::Clip::Png(png))
        }
        linux::Clip::Files(paths) => Some(crate::clip::Clip::Files(paths)),
    }
}

pub fn clipboard_write(clip: &crate::clip::Clip) {
    let native = match clip {
        crate::clip::Clip::Text(text) => linux::Clip::Text(text.clone()),
        crate::clip::Clip::Png(png) => linux::Clip::Png(png.clone()),
        crate::clip::Clip::Files(paths) => linux::Clip::Files(paths.clone()),
    };
    if let Some(Err(error)) = shared_clipboard().map(|clipboard| clipboard.write(&native)) {
        crate::log(&format!("clipboard write failed: {error}"));
    }
}

pub fn ask_yes_no(title: &str, text: &str) -> bool {
    linux::ask_yes_no(title, text).unwrap_or_else(|error| {
        crate::log(&format!("pairing refused: {error}"));
        false
    })
}

/// Sem tela gráfica não há o que controlar: o aviso vai para o log (o systemd religa até a sessão subir).
pub fn prepare() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}
