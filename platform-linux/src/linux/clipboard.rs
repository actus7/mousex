//! Clipboard do X11 pelo arboard. No Wayland (GNOME, KDE) o XWayland sincroniza o clipboard com o
//! do compositor, então o mesmo caminho serve às duas sessões.
use super::{other, x11::X11};
use std::{
    io,
    path::PathBuf,
    sync::{
        Mutex, Once,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use x11rb::{
    connection::Connection,
    protocol::{
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::ConnectionExt as _,
    },
};

/// Quanto esperar o X11 avisar a troca de dono depois de gravarmos (para não devolver a cópia ao servidor).
const OWN_WRITE_WAIT: Duration = Duration::from_millis(300);

pub enum Clip {
    Text(String),
    Png(Vec<u8>),
    Files(Vec<PathBuf>),
}

static SEQ: AtomicU32 = AtomicU32::new(0);
static WATCH: Once = Once::new();

/// Contador de trocas do clipboard (como o changeCount do Mac), mantido por eventos do XFixes.
pub fn clipboard_seq() -> u32 {
    WATCH.call_once(|| {
        std::thread::spawn(|| {
            // ponytail: sem X11 o contador fica parado e o clipboard deste lado nunca vai ao servidor.
            let _ = watch();
        });
    });
    SEQ.load(Ordering::Relaxed)
}

fn watch() -> io::Result<()> {
    let x11 = X11::connect()?;
    x11.conn.xfixes_query_version(5, 0).map_err(other)?.reply().map_err(other)?;
    let clipboard = x11.conn.intern_atom(false, b"CLIPBOARD").map_err(other)?.reply().map_err(other)?.atom;
    let mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    x11.conn.xfixes_select_selection_input(x11.root, clipboard, mask).map_err(other)?;
    x11.conn.flush().map_err(other)?;
    loop {
        x11.conn.wait_for_event().map_err(other)?;
        SEQ.fetch_add(1, Ordering::Relaxed);
    }
}

/// No X11 o conteúdo gravado vive enquanto o dono existe: este objeto fica vivo com o agente.
pub struct Clipboard(Mutex<arboard::Clipboard>);

impl Clipboard {
    pub fn new() -> io::Result<Self> {
        clipboard_seq();
        Ok(Self(Mutex::new(arboard::Clipboard::new().map_err(other)?)))
    }

    /// Arquivos primeiro (o gerenciador de arquivos também põe os caminhos como texto), depois texto, depois imagem.
    pub fn read(&self) -> Option<Clip> {
        let mut clipboard = self.0.lock().ok()?;
        if let Ok(files) = clipboard.get().file_list()
            && !files.is_empty()
        {
            return Some(Clip::Files(files));
        }
        if let Ok(text) = clipboard.get_text() {
            return Some(Clip::Text(text));
        }
        let image = clipboard.get_image().ok()?;
        let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned())?;
        let mut png = Vec::new();
        rgba.write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        Some(Clip::Png(png))
    }

    pub fn write(&self, clip: &Clip) -> io::Result<()> {
        let before = clipboard_seq();
        {
            let mut clipboard = self.0.lock().map_err(other)?;
            match clip {
                Clip::Text(text) => clipboard.set_text(text.as_str()),
                Clip::Files(paths) => clipboard.set().file_list(paths),
                Clip::Png(png) => {
                    let rgba =
                        image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(other)?.to_rgba8();
                    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
                    clipboard.set_image(arboard::ImageData { width, height, bytes: rgba.into_raw().into() })
                }
            }
            .map_err(other)?;
        }
        // O aviso do XFixes chega depois: esperar por ele faz o `clipboard_seq()` seguinte já contar esta cópia.
        let deadline = std::time::Instant::now() + OWN_WRITE_WAIT;
        while clipboard_seq() == before && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_gravado_volta_e_conta_como_troca() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let clipboard = Clipboard::new().expect("clipboard");
        std::thread::sleep(Duration::from_millis(200));
        let before = clipboard_seq();
        clipboard.write(&Clip::Text("olá do MouseX".into())).expect("gravar");
        assert_ne!(clipboard_seq(), before, "a cópia própria precisa contar antes de write voltar");
        match clipboard.read() {
            Some(Clip::Text(text)) => assert_eq!(text, "olá do MouseX"),
            _ => panic!("texto não voltou"),
        }
    }
}
