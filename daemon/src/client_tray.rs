//! Ícone do cliente na barra de menus (Mac) e na bandeja (Linux): mesmos textos e ações nas duas.
//! Só mostra a ligação e abre o log e o "Sobre". Pausar, prender e saltar são do servidor.
use crate::{
    config::data_dir,
    link_status::{self, Link},
};
use mousex_core::i18n::{Lang, lang};
use std::process::Command;

/// Intervalo em que o ícone do Linux confere se a ligação mudou (o do Mac tem o próprio NSTimer).
#[cfg(target_os = "linux")]
const REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

/// "Conectado a <servidor>" ou "Procurando o servidor…".
pub fn status_text(lang: Lang, link: &Link) -> String {
    match link {
        Link::Searching => lang.pick("Procurando o servidor…", "Looking for the server…").into(),
        Link::Connected(server) if server.is_empty() => {
            lang.pick("Conectado ao servidor", "Connected to the server").into()
        }
        Link::Connected(server) => lang.pick(format!("Conectado a {server}"), format!("Connected to {server}")),
    }
}

pub fn log_label(lang: Lang) -> &'static str {
    lang.pick("Abrir log", "Open log")
}

pub fn version_label() -> String {
    format!("MouseX {}", env!("CARGO_PKG_VERSION"))
}

pub fn about_title(lang: Lang) -> &'static str {
    lang.pick("Sobre o MouseX", "About MouseX")
}

pub fn about_text(lang: Lang) -> String {
    mousex_core::about::about_text(
        lang == Lang::Pt,
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_LICENSE"),
        env!("CARGO_PKG_REPOSITORY"),
    )
}

/// Abre o arquivo de log com `open` (Mac) ou `xdg-open` (Linux), em thread própria para não travar o menu.
pub fn open_log() {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let path = data_dir().join("mousex.log");
    let spawned = std::thread::Builder::new().name("mousex-open-log".into()).spawn(move || {
        if let Err(error) = Command::new(program).arg(&path).status() {
            crate::log(&format!("could not open the log with {program}: {error}"));
        }
    });
    if let Err(error) = spawned {
        crate::log(&format!("could not start the log viewer: {error}"));
    }
}

/// Mac: o ícone e o loop do AppKit na thread principal; bloqueia até o processo acabar.
/// Num computador servidor o menu é o do servidor (máquinas e comandos), não o do cliente.
#[cfg(target_os = "macos")]
pub fn run_menu_bar() {
    use crate::config::{Config, Role};
    use mousex_platform_macos::status_item;
    if Config::load(&data_dir()).is_some_and(|config| config.role == Role::Server) {
        return crate::os::run_menu_bar();
    }
    let current = lang();
    status_item::run(
        move || status_text(current, &link_status::get()),
        log_label(current),
        &version_label(),
        open_log,
        move || status_item::show_about(about_title(current), &about_text(current)),
    );
}

/// Linux: ícone StatusNotifierItem e um loop que copia a ligação para o menu. Sem D-Bus, só registra no log.
#[cfg(target_os = "linux")]
pub async fn start() {
    use mousex_platform_linux::tray;
    let current = lang();
    let spawned = tray::start(
        status_text(current, &link_status::get()),
        log_label(current).into(),
        version_label(),
        open_log,
        move || {
            let spawned = std::thread::Builder::new().name("mousex-about".into()).spawn(move || {
                if let Err(error) = tray::show_about(about_title(current), &about_text(current)) {
                    crate::log(&format!("could not show About: {error}"));
                }
            });
            if let Err(error) = spawned {
                crate::log(&format!("could not start About: {error}"));
            }
        },
    )
    .await;
    let tray = match spawned {
        Ok(tray) => tray,
        Err(error) => {
            crate::log(&format!("tray icon unavailable: {error}"));
            return;
        }
    };
    tokio::spawn(async move {
        let mut shown = link_status::get();
        let mut timer = tokio::time::interval(REFRESH);
        loop {
            timer.tick().await;
            if tray.is_closed() {
                break;
            }
            let link = link_status::get();
            if link != shown {
                tray.set_status(status_text(current, &link)).await;
                shown = link;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_diz_servidor_ou_procura() {
        assert_eq!(status_text(Lang::Pt, &Link::Connected("win2".into())), "Conectado a win2");
        assert_eq!(status_text(Lang::En, &Link::Connected("win2".into())), "Connected to win2");
        assert_eq!(status_text(Lang::Pt, &Link::Searching), "Procurando o servidor…");
        assert_eq!(status_text(Lang::En, &Link::Searching), "Looking for the server…");
    }

    #[test]
    fn servidor_antigo_sem_nome_nao_mostra_vazio() {
        assert_eq!(status_text(Lang::Pt, &Link::Connected(String::new())), "Conectado ao servidor");
        assert_eq!(status_text(Lang::En, &Link::Connected(String::new())), "Connected to the server");
    }

    #[test]
    fn rotulos_seguem_o_idioma() {
        assert_eq!(log_label(Lang::Pt), "Abrir log");
        assert_eq!(log_label(Lang::En), "Open log");
        assert_eq!(about_title(Lang::Pt), "Sobre o MouseX");
        assert!(about_text(Lang::En).starts_with(&format!("MouseX {}\n\n", env!("CARGO_PKG_VERSION"))));
    }
}
