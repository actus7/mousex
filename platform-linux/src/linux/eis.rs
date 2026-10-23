//! Wayland: portal RemoteDesktop (versão 2, ConnectToEIS) e libei. O compositor (GNOME 46+, KDE 6.1+)
//! pede permissão uma vez; com "lembrar", o token guardado reabre a sessão sem perguntar.
//! Uma thread própria fala com o portal e o EIS; o injetor só manda comandos por um canal.
use super::{Backend, other};
use crate::{Rect, evdev_button, map_to_regions};
use ashpd::desktop::{
    PersistMode,
    remote_desktop::{ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions},
};
use futures_util::StreamExt;
use reis::{
    ei,
    event::{Connection, Device, DeviceCapability, EiEvent},
};
use std::{
    error::Error,
    io::{self, Write},
    os::unix::{fs::OpenOptionsExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    time::Duration,
};
use tokio::sync::mpsc;

/// Comandos na fila acima disso: movimento e roda são descartados (o próximo movimento corrige);
/// teclas e botões nunca, para nada ficar preso.
const BACKLOG: usize = 256;
/// Prazo para achar o portal no D-Bus (não inclui o usuário responder ao pedido de permissão).
const PORTAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Sem ponteiro absoluto, o primeiro movimento leva o cursor ao canto de cima à esquerda por aqui.
const CORNER: f32 = -100_000.0;

enum Command {
    Key(u16, bool),
    Motion(i32, i32),
    Button(u8, bool),
    Scroll(i32, i32),
}

/// Estado dividido entre o injetor e a thread do EIS.
#[derive(Default)]
struct Shared {
    /// Motivo do fim da sessão, para o erro que o injetor devolve depois.
    ended: Mutex<Option<String>>,
    /// A sessão chegou a funcionar (o usuário permitiu).
    live: AtomicBool,
    /// Comandos enviados e ainda não aplicados.
    pending: AtomicUsize,
    /// Telas como o X11 as vê, para converter posições para as regiões do libei.
    screens: Mutex<Vec<Rect>>,
}

pub(super) struct Eis {
    commands: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}

impl Eis {
    pub(super) fn start(token_file: PathBuf) -> io::Result<Self> {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (ready_tx, ready) = std_mpsc::channel();
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        std::thread::Builder::new().name("mousex-eis".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => return drop(ready_tx.send(Err(error.to_string()))),
            };
            let result = runtime.block_on(run(&token_file, receiver, ready_tx, &thread_shared));
            let text = result.err().map_or_else(|| "remote control session ended".to_string(), |e| e.to_string());
            if let Ok(mut slot) = thread_shared.ended.lock() {
                *slot = Some(text);
            }
        })?;
        match ready.recv_timeout(PORTAL_TIMEOUT) {
            Ok(Ok(())) => Ok(Self { commands, shared }),
            Ok(Err(error)) => Err(other(error)),
            Err(_) => Err(other("the RemoteDesktop portal did not answer")),
        }
    }

    /// Sessão que funcionou e caiu: `BrokenPipe` (vale reiniciar, o token reabre sem perguntar).
    /// Permissão negada ou portal que falhou antes: `PermissionDenied` (reiniciar só repetiria a pergunta).
    fn send(&self, command: Command) -> io::Result<()> {
        let droppable = matches!(command, Command::Motion(..) | Command::Scroll(..));
        if droppable && self.shared.pending.load(Ordering::Relaxed) > BACKLOG {
            return Ok(());
        }
        if self.commands.send(command).is_ok() {
            self.shared.pending.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let reason = self.shared.ended.lock().ok().and_then(|slot| slot.clone());
        let kind = if self.shared.live.load(Ordering::Relaxed) {
            io::ErrorKind::BrokenPipe
        } else {
            io::ErrorKind::PermissionDenied
        };
        Err(io::Error::new(kind, reason.unwrap_or_else(|| "EIS thread ended".into())))
    }
}

impl Backend for Eis {
    fn key(&mut self, evdev: u16, down: bool) -> io::Result<()> {
        self.send(Command::Key(evdev, down))
    }

    fn motion(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.send(Command::Motion(x, y))
    }

    fn button(&mut self, button: u8, down: bool) -> io::Result<()> {
        self.send(Command::Button(button, down))
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> io::Result<()> {
        self.send(Command::Scroll(dx, dy))
    }

    fn set_screens(&mut self, screens: &[Rect]) {
        if let Ok(mut current) = self.shared.screens.lock() {
            *current = screens.to_vec();
        }
    }
}

/// O token reabre a sessão sem perguntar: só o usuário lê.
fn save_token(path: &Path, token: &str) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(token.as_bytes())
}

async fn run(
    token_file: &Path,
    mut commands: mpsc::UnboundedReceiver<Command>,
    ready: std_mpsc::Sender<Result<(), String>>,
    shared: &Shared,
) -> Result<(), Box<dyn Error>> {
    let portal = match RemoteDesktop::new().await {
        Ok(portal) if portal.version() >= 2 => portal,
        Ok(portal) => {
            let error = format!("RemoteDesktop portal version {} has no ConnectToEIS (needs 2)", portal.version());
            let _ = ready.send(Err(error.clone()));
            return Err(error.into());
        }
        Err(error) => {
            let _ = ready.send(Err(format!("RemoteDesktop portal unavailable: {error}")));
            return Err(error.into());
        }
    };
    let _ = ready.send(Ok(()));

    let session = portal.create_session(Default::default()).await?;
    let token = std::fs::read_to_string(token_file).ok();
    let devices = SelectDevicesOptions::default()
        .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(token.as_deref().map(str::trim));
    portal.select_devices(&session, devices).await?;
    let started = portal.start(&session, None, StartOptions::default()).await?.response()?;
    if let Some(token) = started.restore_token() {
        let _ = save_token(token_file, token);
    }
    let fd = portal.connect_to_eis(&session, ConnectToEISOptions::default()).await?;
    let context = ei::Context::new(UnixStream::from(fd))?;
    let (connection, mut events) = context.handshake_tokio("MouseX", ei::handshake::ContextType::Sender).await?;
    shared.live.store(true, Ordering::Relaxed);

    // O que chegou enquanto o usuário decidia já perdeu o sentido.
    while commands.try_recv().is_ok() {
        shared.pending.fetch_sub(1, Ordering::Relaxed);
    }
    let mut emulator = Emulator { connection, devices: Vec::new(), sequence: 0, relative_from: None };
    loop {
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => emulator.handle(event)?,
                Some(Err(error)) => return Err(error.into()),
                None => return Err("the compositor closed the EIS connection".into()),
            },
            command = commands.recv() => match command {
                Some(command) => {
                    shared.pending.fetch_sub(1, Ordering::Relaxed);
                    let screens = shared.screens.lock().map(|s| s.clone()).unwrap_or_default();
                    emulator.apply(command, &screens);
                }
                None => return Ok(()),
            },
        }
        // Buffer do socket cheio (compositor ocupado): o que falta sai no próximo flush.
        if let Err(error) = emulator.connection.flush()
            && io::Error::from(error).kind() != io::ErrorKind::WouldBlock
        {
            return Err(error.into());
        }
    }
}

/// Microssegundos do CLOCK_MONOTONIC, o relógio que o libei usa nos frames.
fn monotonic_micros() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000
}

struct Emulator {
    connection: Connection,
    /// Dispositivos liberados pelo compositor, os de posição absoluta primeiro.
    devices: Vec<Device>,
    sequence: u32,
    /// Sem ponteiro absoluto: a última posição enviada, de onde sai o próximo movimento relativo.
    relative_from: Option<(i32, i32)>,
}

impl Emulator {
    fn handle(&mut self, event: EiEvent) -> Result<(), Box<dyn Error>> {
        match event {
            EiEvent::SeatAdded(added) => added.seat.bind_capabilities(
                DeviceCapability::PointerAbsolute
                    | DeviceCapability::Pointer
                    | DeviceCapability::Keyboard
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll,
            ),
            EiEvent::DeviceResumed(resumed) => {
                resumed.device.device().start_emulating(self.connection.serial(), self.sequence);
                self.sequence = self.sequence.wrapping_add(1);
                self.devices.retain(|device| device != &resumed.device);
                self.devices.push(resumed.device);
                self.devices.sort_by_key(|device| !device.has_capability(DeviceCapability::PointerAbsolute));
                if !self.devices.iter().any(|device| device.has_capability(DeviceCapability::PointerAbsolute)) {
                    eprintln!("MouseX: the compositor offers no absolute pointer; using relative motion");
                }
            }
            EiEvent::DevicePaused(paused) => self.devices.retain(|device| device != &paused.device),
            EiEvent::DeviceRemoved(removed) => self.devices.retain(|device| device != &removed.device),
            EiEvent::Disconnected(_) => return Err("the compositor ended the remote control session".into()),
            _ => {}
        }
        Ok(())
    }

    fn find<T: ei::Interface>(&self) -> Option<(&Device, T)> {
        self.devices.iter().find_map(|device| Some((device, device.interface::<T>()?)))
    }

    fn apply(&mut self, command: Command, screens: &[Rect]) {
        let device = match command {
            Command::Key(code, down) => self.find::<ei::Keyboard>().map(|(device, keyboard)| {
                let state = if down { ei::keyboard::KeyState::Press } else { ei::keyboard::KeyState::Released };
                keyboard.key(code.into(), state);
                device.clone()
            }),
            Command::Motion(x, y) => self.motion(x, y, screens),
            Command::Button(button, down) => self.find::<ei::Button>().and_then(|(device, buttons)| {
                let state = if down { ei::button::ButtonState::Press } else { ei::button::ButtonState::Released };
                buttons.button(evdev_button(button)?, state);
                Some(device.clone())
            }),
            // libei: 120 por dente como no Windows, mas positivo é para baixo.
            Command::Scroll(dx, dy) => self.find::<ei::Scroll>().map(|(device, scroll)| {
                scroll.scroll_discrete(dx, -dy);
                device.clone()
            }),
        };
        if let Some(device) = device {
            device.device().frame(self.connection.serial(), monotonic_micros());
        }
    }

    fn motion(&mut self, x: i32, y: i32, screens: &[Rect]) -> Option<Device> {
        if let Some((device, pointer)) = self.find::<ei::PointerAbsolute>() {
            let regions: Vec<Rect> = device
                .regions()
                .iter()
                .map(|r| Rect { x: r.x as i32, y: r.y as i32, width: r.width as i32, height: r.height as i32 })
                .collect();
            let (x, y) = map_to_regions(x, y, screens, &regions)?;
            pointer.motion_absolute(x as f32, y as f32);
            return Some(device.clone());
        }
        // ponytail: relativo a partir da última posição enviada; o compositor pode acelerar o movimento e
        // o usuário pode mexer o cursor local, então a posição deriva até o próximo "canto". Pedir uma
        // sessão de ScreenCast (que dá regiões absolutas) se isso aparecer.
        let from = self.relative_from.replace((x, y));
        let (device, pointer) = self.find::<ei::Pointer>()?;
        if from.is_none() {
            pointer.motion_relative(CORNER, CORNER);
        }
        let (from_x, from_y) = from.unwrap_or((0, 0));
        pointer.motion_relative((x - from_x) as f32, (y - from_y) as f32);
        Some(device.clone())
    }
}
