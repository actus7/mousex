//! Ícone na bandeja pelo StatusNotifierItem (crate `ksni`, D-Bus) e a janela "Sobre".
//! `ksni` fala o protocolo que o KDE já usa e que o GNOME usa com a extensão AppIndicator; sem D-Bus de sessão,
//! `start` devolve erro e o agente segue sem ícone. Não há nada de servidor aqui.
use ksni::TrayMethods;
use std::{
    io,
    process::{Command, Stdio},
    sync::Arc,
};

const ICON_PNG: &[u8] = include_bytes!("../../assets/mousex.png");

struct ClientTray {
    status: String,
    log_label: String,
    version_label: String,
    icon: Vec<ksni::Icon>,
    on_log: Arc<dyn Fn() + Send + Sync>,
    on_about: Arc<dyn Fn() + Send + Sync>,
}

impl ksni::Tray for ClientTray {
    fn id(&self) -> String {
        "mousex-client".into()
    }

    fn title(&self) -> String {
        "MouseX".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem { label: self.status.clone(), enabled: false, ..Default::default() }.into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: self.log_label.clone(),
                activate: Box::new(|tray: &mut Self| (tray.on_log)()),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: self.version_label.clone(),
                activate: Box::new(|tray: &mut Self| (tray.on_about)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Ícone vivo: `set_status` troca a linha de status do menu.
pub struct Tray(ksni::Handle<ClientTray>);

impl Tray {
    pub async fn set_status(&self, status: String) {
        // None só quando a bandeja já foi encerrada; quem chama para o laço com `is_closed`.
        let _ = self.0.update(|tray| tray.status = status).await;
    }

    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// Decodifica o PNG embutido para o formato ARGB32 (um pixel por 4 bytes, alfa primeiro) que o D-Bus pede.
fn icon() -> io::Result<Vec<ksni::Icon>> {
    let image = image::load_from_memory(ICON_PNG).map_err(io::Error::other)?.to_rgba8();
    let (width, height) = (image.width(), image.height());
    let data = image.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    Ok(vec![ksni::Icon { width: width as i32, height: height as i32, data }])
}

/// Registra o ícone. `on_log` e `on_about` rodam na thread do D-Bus: quem bloqueia deve criar a própria thread.
pub async fn start(
    status: String,
    log_label: String,
    version_label: String,
    on_log: impl Fn() + Send + Sync + 'static,
    on_about: impl Fn() + Send + Sync + 'static,
) -> io::Result<Tray> {
    let tray = ClientTray {
        status,
        log_label,
        version_label,
        icon: icon()?,
        on_log: Arc::new(on_log),
        on_about: Arc::new(on_about),
    };
    let handle = tray.spawn().await.map_err(|error| io::Error::other(error.to_string()))?;
    Ok(Tray(handle))
}

/// "Sobre" em janela: zenity, depois kdialog; se nenhum existir, a notificação do sistema.
pub fn show_about(title: &str, text: &str) -> io::Result<()> {
    let attempts: [(&str, &[&str]); 2] = [
        ("zenity", &["--info", "--no-markup", "--title", title, "--text", text]),
        ("kdialog", &["--title", title, "--msgbox", text]),
    ];
    let mut failures = Vec::new();
    for (program, args) in attempts {
        match Command::new(program).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
            Ok(status) if matches!(status.code(), Some(0 | 1)) => return Ok(()),
            Ok(status) => failures.push(format!("{program} exited with {status}")),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }
    let notified = Command::new("notify-send")
        .args(["--app-name=MouseX", title, text])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if notified.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "no About window or notification worked ({}); install zenity or kdialog",
        failures.join(", ")
    )))
}
