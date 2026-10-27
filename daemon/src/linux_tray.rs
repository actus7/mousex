//! Tray icon for the Linux server, same menu as Windows.
//! The default build has no icon: this machine has no libdbus headers, and
//! `mousex monitor` is the status window. `--features tray` builds the icon.
use crate::os::{Event, TrayStatus};
use std::{io, sync::mpsc};

#[cfg(not(feature = "tray"))]
pub fn start(_tx: mpsc::Sender<Event>) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "status window is `mousex monitor`"))
}

#[cfg(not(feature = "tray"))]
pub fn set_status(_status: TrayStatus) {}

#[cfg(feature = "tray")]
pub use tray::{set_status, start};

#[cfg(feature = "tray")]
mod tray {
    use crate::os::{Event, TrayCommand, TrayStatus};
    use ksni::{MenuItem, Tray, menu::StandardItem};
    use std::{
        io,
        sync::{OnceLock, mpsc},
    };

    struct Icon {
        tx: mpsc::Sender<Event>,
        status: TrayStatus,
    }

    impl Tray for Icon {
        fn id(&self) -> String {
            "mousex".into()
        }

        fn title(&self) -> String {
            "MouseX".into()
        }

        fn icon_name(&self) -> String {
            if self.status.active && !self.status.paused { "input-mouse".into() } else { "input-mouse-symbolic".into() }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let t = |pt: &str, en: &str| if self.status.portuguese { pt.to_string() } else { en.to_string() };
            let mut items = Vec::new();
            for (index, (name, connected)) in self.status.machines.iter().take(9).enumerate() {
                let mark = if *connected { "●" } else { "○" };
                let number = (index + 1) as u8;
                let enabled = *connected && !self.status.paused;
                items.push(item(
                    format!("{mark}  {name}\tCtrl+Alt+Shift+{number}"),
                    enabled,
                    Box::new(move |icon: &mut Icon| {
                        let _ = icon.tx.send(Event::Tray(TrayCommand::Jump(number)));
                    }),
                ));
            }
            if !items.is_empty() {
                items.push(MenuItem::Separator);
            }
            let paused = self.status.paused;
            items.push(item(
                t("Pausar compartilhamento", "Pause sharing"),
                true,
                Box::new(|icon| {
                    let _ = icon.tx.send(Event::Tray(TrayCommand::Pause));
                }),
            ));
            let _ = paused;
            items.push(item(
                t("Prender cursor na tela", "Lock cursor to screen"),
                true,
                Box::new(|icon| {
                    let _ = icon.tx.send(Event::Tray(TrayCommand::LockCursor));
                }),
            ));
            items.push(item(
                t("Reconectar máquinas", "Reconnect computers"),
                true,
                Box::new(|icon| {
                    let _ = icon.tx.send(Event::Tray(TrayCommand::Reconnect));
                }),
            ));
            items.push(MenuItem::Separator);
            items.push(item(
                t("Arrumar telas…", "Arrange screens…"),
                true,
                Box::new(|icon| {
                    let _ = icon.tx.send(Event::Tray(TrayCommand::Arrange));
                }),
            ));
            items.push(item(
                t("Abrir log", "Open log"),
                true,
                Box::new(|icon| {
                    let _ = icon.tx.send(Event::Tray(TrayCommand::ShowLog));
                }),
            ));
            items
        }
    }

    fn item(label: String, enabled: bool, activate: Box<dyn Fn(&mut Icon) + Send>) -> MenuItem<Icon> {
        MenuItem::Standard(StandardItem {
            label,
            icon_name: String::new(),
            enabled,
            visible: true,
            disposition: Default::default(),
            activate,
        })
    }

    static HANDLE: OnceLock<ksni::Handle<Icon>> = OnceLock::new();

    pub fn start(tx: mpsc::Sender<Event>) -> io::Result<()> {
        let service = ksni::TrayService::new(Icon { tx, status: TrayStatus::default() });
        let handle = service.handle();
        service.spawn();
        let _ = HANDLE.set(handle);
        Ok(())
    }

    pub fn set_status(status: TrayStatus) {
        if let Some(handle) = HANDLE.get() {
            handle.update(|icon| icon.status = status);
        }
    }
}
