//! Papel servidor: captura a entrada física, roteia pelo cursor virtual e fala com cada cliente.
use crate::clip::Clip;
use crate::platform::{clipboard_read, clipboard_write};
use crate::{
    clip,
    layout::{connected_view, home_arrangement, native_id, rebuild},
    log,
    platform::{button_to_u8, local_monitors},
};
use mousex_core::{
    ScreenLayout,
    identity::DeviceIdentity,
    keymap,
    net::{self, Link},
    pairing::{Allowlist, PeerRecord, pairing_code},
    proto::{Control, MonitorInfo, Motion, read_control, write_control},
    router::{Action, Router, machines_in_order, plausible_delta},
};
use mousex_native::{Event, Hooks, Hotkey, Injector, TrayCommand, TrayStatus};
use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// A cada intervalo, se houve entrada física há pouco, as outras máquinas são mantidas acordadas.
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(30);
const ACTIVE_WINDOW: Duration = Duration::from_secs(60);
/// Bloqueio com entrada física recente foi pedido pelo usuário (Win+L); mais antigo é inatividade, e aí o
/// usuário pode estar usando o teclado de outra máquina: ela não é bloqueada.
const LOCK_INTENT_WINDOW: Duration = Duration::from_secs(10);

fn user_locked(since_input: Duration) -> bool {
    since_input <= LOCK_INTENT_WINDOW
}
/// Recusa lembrada: o mesmo par não reabre o diálogo durante esse tempo.
const REJECTION_MEMORY: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
struct Rejections(HashMap<String, Instant>);

impl Rejections {
    fn recent(&self, fingerprint: &str, now: Instant) -> bool {
        self.0.get(fingerprint).is_some_and(|&at| now.saturating_duration_since(at) < REJECTION_MEMORY)
    }
    fn reject(&mut self, fingerprint: &str, now: Instant) {
        self.0.retain(|_, &mut at| now.saturating_duration_since(at) < REJECTION_MEMORY);
        self.0.insert(fingerprint.into(), now);
    }
}

static REJECTIONS: LazyLock<Mutex<Rejections>> = LazyLock::new(Mutex::default);
/// Um diálogo de pareamento por vez; os demais pedidos são recusados sem abrir janela.
static PAIRING_OPEN: AtomicBool = AtomicBool::new(false);
/// Scancodes (set 1) usados pela hotkey de emergência Ctrl+Alt+Shift+Home.
const CTRL: u16 = 0x1D;
const ALT: u16 = 0x38;
const LSHIFT: u16 = 0x2A;
const RSHIFT: u16 = 0x36;
const HOME: (u16, bool) = (0x47, true);

enum Msg {
    Input(Event),
    Connected {
        machine: String,
        client: Client,
    },
    Monitors {
        machine: String,
        monitors: Vec<MonitorInfo>,
    },
    Disconnected {
        machine: String,
        id: usize,
    },
    Clipboard {
        from: String,
        clip: Clip,
    },
    /// Editor fechado: o layout salvo pode ter mudado.
    LayoutEdited,
    /// Relógio da atividade (manter as outras máquinas acordadas).
    Tick,
}

/// Editor de telas instalado ao lado do agente.
const EDITOR: &str = "mousex-editor.exe";

struct Client {
    conn: quinn::Connection,
    control: mpsc::UnboundedSender<Control>,
    /// Ids nativos na ordem do Hello: o índice vai no datagrama de movimento.
    ids: Vec<String>,
    /// Monitores do último Hello: reconstruir o layout nunca perde um cliente conectado.
    monitors: Vec<MonitorInfo>,
    seq: u32,
}

impl Client {
    fn send(&self, message: Control) {
        let _ = self.control.send(message);
    }
}

pub async fn run(machine: String, identity: DeviceIdentity, dir: PathBuf) -> io::Result<()> {
    let endpoint = net::server_endpoint(&identity, net::PORT)?;
    let _advertiser = mousex_core::discovery::Advertiser::start(&identity.fingerprint, &machine, net::PORT)?;
    let (_hooks, events) = Hooks::start()?;
    if let Err(error) = mousex_native::start_tray() {
        log(&format!("tray icon unavailable: {error}"));
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    let hook_tx = tx.clone();
    // Ponte da thread dos hooks (canal std) para o loop tokio; o hook em si nunca espera.
    std::thread::Builder::new().name("mousex-ponte".into()).spawn(move || {
        while let Ok(event) = events.recv() {
            if hook_tx.send(Msg::Input(event)).is_err() {
                break;
            }
        }
    })?;
    tokio::spawn(accept_loop(endpoint, tx.clone(), dir.clone(), machine.clone()));
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ACTIVITY_INTERVAL);
        loop {
            ticker.tick().await;
            if tick_tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
    log(&format!("server {machine} listening on port {}", net::PORT));
    let mut state = State::new(machine, dir.join("layout.json"), tx);
    state.update_tray();
    while let Some(message) = rx.recv().await {
        state.handle(message);
    }
    Ok(())
}

async fn accept_loop(endpoint: quinn::Endpoint, tx: mpsc::UnboundedSender<Msg>, dir: PathBuf, home: String) {
    loop {
        match net::accept(&endpoint).await {
            Ok(link) => {
                tokio::spawn(serve_client(link, tx.clone(), dir.clone(), home.clone()));
            }
            Err(net::NetError::Closed) => break,
            Err(error) => log(&format!("connection refused: {error}")),
        }
    }
}

/// Autoriza (allowlist ou pareamento com diálogo) e cuida das tasks de I/O de um cliente.
async fn serve_client(link: Link, tx: mpsc::UnboundedSender<Msg>, dir: PathBuf, home: String) {
    let Link { conn, peer_fingerprint, mut control_tx, mut control_rx } = link;
    let Ok(Ok(Control::Hello { machine: claimed, monitors })) =
        tokio::time::timeout(HELLO_TIMEOUT, read_control(&mut control_rx)).await
    else {
        conn.close(1u32.into(), b"hello ausente");
        return;
    };
    // Identidade vinculada ao certificado: o nome do Hello só vale no primeiro pareamento.
    let Some(machine) = authorize(&dir, &conn, &peer_fingerprint, &claimed, &home).await else {
        log(&format!("pairing refused: {claimed}"));
        conn.close(crate::client::REJECTED.into(), b"recusado");
        return;
    };
    log(&format!("client connected: {machine}"));
    let (control, mut outgoing) = mpsc::unbounded_channel::<Control>();
    // Escritor único do stream de controle: o loop de roteamento nunca espera a rede.
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if write_control(&mut control_tx, &message).await.is_err() {
                break;
            }
        }
    });
    let id = conn.stable_id();
    let ids = monitors.iter().map(|m| m.id.clone()).collect();
    let _ = tx.send(Msg::Connected {
        machine: machine.clone(),
        client: Client { conn: conn.clone(), control, ids, monitors, seq: 0 },
    });
    let clip_conn = conn.clone();
    let clip_tx = tx.clone();
    let clip_machine = machine.clone();
    let clip_task = tokio::spawn(async move {
        while let Ok(stream) = clip_conn.accept_uni().await {
            let (tx, from) = (clip_tx.clone(), clip_machine.clone());
            tokio::spawn(async move {
                if let Some(clip) = clip::receive(stream, &clip::inbox()).await {
                    let _ = tx.send(Msg::Clipboard { from, clip });
                }
            });
        }
    });
    while let Ok(message) = mousex_core::proto::read_known(&mut control_rx).await {
        if let Control::Hello { monitors, .. } = message {
            let _ = tx.send(Msg::Monitors { machine: machine.clone(), monitors });
        }
    }
    clip_task.abort();
    log(&format!("client disconnected: {machine}"));
    let _ = tx.send(Msg::Disconnected { machine, id });
}

/// Nome vinculado ao fingerprint no pareamento; `None` se o par não está autorizado.
fn bound_name(peers: &[PeerRecord], fingerprint: &str, _claimed: &str) -> Option<String> {
    peers.iter().find(|p| p.fingerprint == fingerprint).map(|p| p.name.clone())
}

/// Nome livre para um par novo: duas máquinas nunca dividem a mesma identidade.
fn unique_name(peers: &[PeerRecord], claimed: &str, home: &str) -> String {
    // O nome da casa é reservado: um cliente com ele seria tratado como monitor da casa.
    let taken = |name: &str| name == home || peers.iter().any(|p| p.name == name);
    if !taken(claimed) {
        return claimed.into();
    }
    (2..).map(|n| format!("{claimed}-{n}")).find(|name| !taken(name)).expect("sequência infinita")
}

/// Par conhecido → nome do pareamento. Par novo → diálogo com código; aceito, grava com nome único.
async fn authorize(
    dir: &Path,
    conn: &quinn::Connection,
    fingerprint: &str,
    claimed: &str,
    home: &str,
) -> Option<String> {
    let mut allowlist = Allowlist::load(dir).ok()?;
    if let Some(name) = bound_name(allowlist.list(), fingerprint, claimed) {
        return Some(name);
    }
    if REJECTIONS.lock().is_ok_and(|r| r.recent(fingerprint, Instant::now())) {
        return None;
    }
    if PAIRING_OPEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    let name = unique_name(allowlist.list(), claimed, home);
    let code = pairing_code(&net::pairing_secret(conn), fingerprint);
    let text = mousex_core::i18n::lang().pick(
        format!("Permitir que \"{name}\" seja controlado pelo teclado e pelo mouse deste computador?\n\nCódigo: {code:06}\n\nConfirme somente se o mesmo código aparece no outro computador."),
        format!("Allow \"{name}\" to be controlled by this computer's keyboard and mouse?\n\nCode: {code:06}\n\nConfirm only if the same code is shown on the other computer."),
    );
    let title = mousex_core::i18n::tr("MouseX — pareamento", "MouseX — pairing");
    let accepted = tokio::task::spawn_blocking(move || mousex_native::ask_yes_no(title, &text)).await.unwrap_or(false);
    PAIRING_OPEN.store(false, Ordering::Release);
    if !accepted {
        if let Ok(mut rejections) = REJECTIONS.lock() {
            rejections.reject(fingerprint, Instant::now());
        }
        return None;
    }
    allowlist.confirm(PeerRecord { fingerprint: fingerprint.into(), name: name.clone() }, true).is_ok().then_some(name)
}

struct State {
    home: String,
    layout_path: PathBuf,
    layout: ScreenLayout,
    router: Router,
    clients: HashMap<String, Client>,
    remote: Option<String>,
    /// Monitor (id global) e posição local atuais do cursor remoto, para cliques.
    cursor: (String, i32, i32),
    held: BTreeSet<(u16, bool)>,
    held_buttons: BTreeSet<mousex_native::Button>,
    local: Injector,
    /// Compartilhado com as tasks bloqueantes de clipboard (leitura e escrita fora do loop).
    home_clip_seq: Arc<AtomicU32>,
    /// Clipboard da casa quando o cursor voltou: resposta tardia do remoto não sobrescreve cópia mais nova.
    pull_seq: u32,
    tx: mpsc::UnboundedSender<Msg>,
    editor_open: Arc<AtomicBool>,
    /// Compartilhamento pausado pelo menu: o cursor não sai de casa.
    paused: bool,
    /// Última entrada física (teclado ou mouse), para manter as outras máquinas acordadas.
    last_input: Instant,
    /// Sessão bloqueada: ninguém é mantido acordado até a próxima entrada física (após o desbloqueio).
    session_locked: bool,
}

impl State {
    fn new(home: String, layout_path: PathBuf, tx: mpsc::UnboundedSender<Msg>) -> Self {
        let layout = rebuild(&read_saved(&layout_path), &home, &local_monitors(), &[]);
        let router = Router::new(connected_view(&layout, &home, &[]), &home);
        Self {
            home,
            layout_path,
            layout,
            router,
            clients: HashMap::new(),
            remote: None,
            cursor: (String::new(), 0, 0),
            held: BTreeSet::new(),
            held_buttons: BTreeSet::new(),
            local: Injector::default(),
            home_clip_seq: Arc::new(AtomicU32::new(mousex_native::clipboard_seq())),
            pull_seq: mousex_native::clipboard_seq(),
            tx,
            editor_open: Arc::new(AtomicBool::new(false)),
            paused: false,
            last_input: Instant::now(),
            session_locked: false,
        }
    }

    fn handle(&mut self, message: Msg) {
        match message {
            Msg::Input(event) => self.input(event),
            Msg::Connected { machine, client } => {
                self.clients.insert(machine, client);
                self.rebuild_layout();
            }
            Msg::Monitors { machine, monitors } => {
                if let Some(client) = self.clients.get_mut(&machine) {
                    client.ids = monitors.iter().map(|m| m.id.clone()).collect();
                    client.monitors = monitors;
                }
                self.rebuild_layout();
            }
            Msg::Disconnected { machine, id } => {
                if self.clients.get(&machine).is_some_and(|c| c.conn.stable_id() == id) {
                    self.clients.remove(&machine);
                    let action = self.router.on_disconnect(&machine);
                    self.apply(action);
                    self.refresh_router();
                }
            }
            Msg::Tick => self.keep_others_awake(),
            Msg::LayoutEdited => {
                self.apply_home_arrangement();
                self.rebuild_layout();
                log("layout reloaded after the editor closed");
            }
            Msg::Clipboard { from, clip } => match &self.remote {
                None if mousex_native::clipboard_seq() != self.pull_seq => {
                    log("remote clipboard dropped: a newer copy was made on this computer");
                }
                None => {
                    // Gravação bloqueante fora do loop de roteamento.
                    let seq = self.home_clip_seq.clone();
                    tokio::task::spawn_blocking(move || {
                        clipboard_write(&clip);
                        seq.store(mousex_native::clipboard_seq(), Ordering::Relaxed);
                    });
                }
                Some(target) if *target != from => {
                    if let Some(client) = self.clients.get(target) {
                        let conn = client.conn.clone();
                        tokio::spawn(async move { clip::send(&conn, &clip).await });
                    }
                }
                Some(_) => {}
            },
        }
    }

    fn input(&mut self, event: Event) {
        if matches!(
            event,
            Event::Key { .. } | Event::Move { .. } | Event::Delta { .. } | Event::Button { .. } | Event::Wheel { .. }
        ) {
            self.last_input = Instant::now();
            self.session_locked = false;
        }
        match event {
            Event::Key { scancode, extended, down } => {
                if down {
                    self.held.insert((scancode, extended));
                } else {
                    self.held.remove(&(scancode, extended));
                }
                if down && (scancode, extended) == HOME && self.hotkey_modifiers() {
                    log("emergency hotkey");
                    let action = self.router.force_home();
                    self.apply(action);
                    return;
                }
                if let Some(client) = self.focused() {
                    match keymap::win_scancode_to_hid(scancode, extended) {
                        Some(hid) => client.send(Control::Key { hid, down }),
                        // Tecla fora da tabela não chega ao remoto: deixa rastro para corrigir o mapa.
                        None if down => log(&format!("unmapped key: scancode {scancode:#04x} extended={extended}")),
                        None => {}
                    }
                }
            }
            // Move só conta em casa; Delta só com foco remoto. Eventos atrasados do outro modo são descartados.
            Event::Move { x, y } => {
                if self.remote.is_none() {
                    let action = self.router.on_home_cursor(x, y);
                    self.apply(action);
                }
            }
            Event::Delta { dx, dy } => {
                if self.remote.is_some() && (dx, dy) != (0, 0) && plausible_delta(dx, dy) {
                    let action = self.router.on_delta(dx, dy);
                    self.apply(action);
                }
            }
            Event::Tray(TrayCommand::Arrange) => self.open_editor(),
            Event::Hotkey(Hotkey::Jump(n)) | Event::Tray(TrayCommand::Jump(n)) => self.jump(n),
            Event::Hotkey(Hotkey::ToggleLock) | Event::Tray(TrayCommand::LockCursor) => {
                let locked = !self.router.locked();
                self.router.set_locked(locked);
                log(if locked { "cursor locked to the current computer" } else { "cursor unlocked" });
                self.update_tray();
            }
            Event::Tray(TrayCommand::Pause) => {
                self.paused = !self.paused;
                if self.paused {
                    let action = self.router.force_home();
                    self.apply(action);
                }
                log(if self.paused { "sharing paused" } else { "sharing resumed" });
                self.update_tray();
            }
            Event::Tray(TrayCommand::Reconnect) => {
                // As máquinas reconectam sozinhas em instantes (backoff curto do cliente).
                log("reconnecting computers");
                for client in self.clients.values() {
                    client.conn.close(0u32.into(), b"reconectar");
                }
            }
            Event::SessionLocked => {
                self.session_locked = true;
                if user_locked(self.last_input.elapsed()) {
                    log("session locked by the user: locking the other computers");
                    for client in self.clients.values() {
                        client.send(Control::Lock);
                    }
                } else {
                    log("session locked after inactivity: other computers keep their own timeout");
                }
            }
            Event::Tray(TrayCommand::ShowLog) => {
                let _ =
                    std::process::Command::new("notepad.exe").arg(crate::config::data_dir().join("mousex.log")).spawn();
            }
            Event::DisplayChange => {
                // Monitores da casa mudaram: bordas recalculadas com as coordenadas novas.
                self.rebuild_layout();
                log("this computer's monitors changed; layout reloaded");
            }
            Event::DesktopSwitch => {
                // Bloqueio, Ctrl+Alt+Del ou UAC: os key-ups não virão; esquece o estado e volta para casa.
                log("desktop switch: key state cleared");
                self.held.clear();
                self.held_buttons.clear();
                let action = self.router.force_home();
                self.apply(action);
            }
            Event::Button { button, down } => {
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                let (monitor_id, x, y) = self.cursor.clone();
                if let Some(client) = self.focused() {
                    client.send(Control::Button {
                        button: button_to_u8(button),
                        down,
                        monitor_id: native_id(&monitor_id).into(),
                        x,
                        y,
                    });
                }
            }
            Event::Wheel { delta, horizontal } => {
                let (dx, dy) = if horizontal { (delta as i32, 0) } else { (0, delta as i32) };
                if let Some(client) = self.focused() {
                    client.send(Control::Wheel { dx, dy });
                }
            }
        }
    }

    fn hotkey_modifiers(&self) -> bool {
        let held = |code: u16| self.held.contains(&(code, false)) || self.held.contains(&(code, true));
        held(CTRL) && held(ALT) && (held(LSHIFT) || held(RSHIFT))
    }

    fn focused(&self) -> Option<&Client> {
        self.remote.as_ref().and_then(|machine| self.clients.get(machine))
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Enter { machine, monitor_id, local_x, local_y } => {
                // Botão pressionado (arrastando) não atravessa: o button-up seria suprimido e o botão ficaria preso aqui.
                if !self.clients.contains_key(&machine) || !self.held_buttons.is_empty() || self.paused {
                    self.router.force_home();
                    return;
                }
                // Teclas seguradas em casa seriam soltas só no remoto: libera aqui antes de suprimir.
                for &(scancode, extended) in &self.held {
                    let _ = self.local.send(Event::Key { scancode, extended, down: false });
                }
                // Supressão antes de estacionar: daqui em diante o cursor real só se move por SetCursorPos.
                Hooks::set_suppress(true);
                let park = mousex_native::park_cursor();
                log(&format!("entered {machine} ({monitor_id} {local_x},{local_y}); cursor parked at {park:?}"));
                self.enter(&machine, &monitor_id, local_x, local_y);
                let seq = mousex_native::clipboard_seq();
                if seq != self.home_clip_seq.load(Ordering::Relaxed)
                    && let Some(client) = self.clients.get(&machine)
                {
                    // Leitura (DIB→PNG, renderização atrasada do Office) nunca no loop com a supressão ligada.
                    // A sequência só é marcada como enviada depois de uma leitura bem-sucedida.
                    let (conn, sent) = (client.conn.clone(), self.home_clip_seq.clone());
                    tokio::spawn(async move {
                        if let Ok(Some(clip)) = tokio::task::spawn_blocking(clipboard_read).await {
                            sent.store(seq, Ordering::Relaxed);
                            clip::send(&conn, &clip).await;
                        }
                    });
                }
            }
            Action::Move { machine, monitor_id, local_x, local_y } => {
                self.cursor = (monitor_id.clone(), local_x, local_y);
                if let Some(client) = self.clients.get_mut(&machine)
                    && let Some(index) = client.ids.iter().position(|id| id == native_id(&monitor_id))
                {
                    client.seq = client.seq.wrapping_add(1);
                    let motion = Motion { seq: client.seq, monitor: index as u16, x: local_x, y: local_y };
                    let _ = client.conn.send_datagram(motion.encode().to_vec().into());
                }
            }
            Action::Switch { from, to, monitor_id, local_x, local_y } => {
                self.leave(&from);
                self.enter(&to, &monitor_id, local_x, local_y);
            }
            Action::ReturnHome { global_x, global_y } => {
                if let Some(previous) = self.remote.take() {
                    self.pull_seq = mousex_native::clipboard_seq();
                    self.leave(&previous);
                    log(&format!("returned from {previous} to ({global_x},{global_y})"));
                }
                Hooks::set_suppress(false);
                mousex_native::set_cursor(global_x, global_y);
            }
        }
    }

    fn enter(&mut self, machine: &str, monitor_id: &str, x: i32, y: i32) {
        self.remote = Some(machine.into());
        self.cursor = (monitor_id.into(), x, y);
        if let Some(client) = self.clients.get(machine) {
            client.send(Control::Enter { monitor_id: native_id(monitor_id).into(), x, y });
        }
    }

    /// Solta tudo no cliente e pede o clipboard dele (só vem se mudou).
    fn leave(&mut self, machine: &str) {
        if let Some(client) = self.clients.get(machine) {
            client.send(Control::Leave);
            client.send(Control::ClipboardPull);
        }
    }

    /// Relê o layout salvo (o editor ou outra conexão pode tê-lo mudado), aplica a casa real e os clientes
    /// conectados, grava e atualiza o roteador. Única porta de entrada: nenhum caminho perde um cliente conectado.
    fn rebuild_layout(&mut self) {
        self.layout =
            rebuild(&read_saved(&self.layout_path), &self.home, &local_monitors(), &self.connected_monitors());
        if let Err(error) = save_layout(&self.layout_path, &self.layout) {
            log(&format!("failed to save layout: {error}"));
        }
        self.refresh_router();
    }

    /// Telas da casa movidas no editor: o Windows passa a usar a mesma arrumação, e o layout salvo é
    /// deslocado para bater com as coordenadas reais (a principal fica em 0,0).
    fn apply_home_arrangement(&self) {
        let Some(plan) = home_arrangement(&read_saved(&self.layout_path), &self.home, &local_monitors()) else {
            return;
        };
        if !plan.positions.is_empty() {
            if let Err(error) = mousex_native::set_monitor_positions(&plan.positions) {
                log(&format!("failed to rearrange this computer's screens: {error}"));
                return;
            }
            log("this computer's screens were rearranged to match the layout");
        }
        if let Err(error) = save_layout(&self.layout_path, &plan.layout) {
            log(&format!("failed to save layout: {error}"));
        }
    }

    /// Abre o editor (um por vez) e recarrega o layout quando ele fechar.
    fn open_editor(&mut self) {
        if self.editor_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let editor = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(EDITOR)));
        let Some(editor) = editor.filter(|path| path.exists()) else {
            log("screen editor not installed");
            self.editor_open.store(false, Ordering::Release);
            return;
        };
        let (layout, tx, open, home) =
            (self.layout_path.clone(), self.tx.clone(), self.editor_open.clone(), self.home.clone());
        tokio::task::spawn_blocking(move || {
            match std::process::Command::new(&editor).arg("--layout").arg(&layout).arg("--home").arg(&home).status() {
                Ok(_) => {
                    let _ = tx.send(Msg::LayoutEdited);
                }
                Err(error) => log(&format!("failed to open the editor: {error}")),
            }
            open.store(false, Ordering::Release);
        });
    }

    fn connected_monitors(&self) -> Vec<(String, Vec<MonitorInfo>)> {
        self.clients.iter().map(|(name, c)| (name.clone(), c.monitors.clone())).collect()
    }

    fn refresh_router(&mut self) {
        self.router.set_layout(connected_view(&self.layout, &self.home, &self.connected_monitors()));
        self.update_tray();
    }

    /// Ctrl+Alt+Shift+N ou item N do menu: N-ésima máquina da esquerda para a direita (casa incluída).
    fn jump(&mut self, n: u8) {
        if self.paused {
            return;
        }
        let order = machines_in_order(&self.layout);
        let Some(target) = (n as usize).checked_sub(1).and_then(|i| order.get(i)).cloned() else { return };
        if target != self.home && !self.clients.contains_key(&target) {
            return;
        }
        let action = self.router.jump_to(&target);
        self.apply(action);
    }

    /// Usuário ativo há pouco: esta máquina e as outras não bloqueiam a tela por inatividade.
    /// Quando ele sai, ninguém é mantido acordado e cada uma bloqueia no seu tempo normal.
    fn keep_others_awake(&self) {
        if self.session_locked || self.last_input.elapsed() > ACTIVE_WINDOW {
            return;
        }
        mousex_native::keep_awake();
        for client in self.clients.values() {
            client.send(Control::Activity);
        }
    }

    fn update_tray(&self) {
        let machines = machines_in_order(&self.layout)
            .into_iter()
            .map(|name| {
                let connected = name == self.home || self.clients.contains_key(&name);
                (name, connected)
            })
            .collect();
        mousex_native::set_tray_status(TrayStatus {
            machines,
            active: !self.clients.is_empty(),
            paused: self.paused,
            locked: self.router.locked(),
            portuguese: mousex_core::i18n::lang() == mousex_core::i18n::Lang::Pt,
        });
    }
}

/// Layout como está no disco; ausente ou ilegível vira vazio (a casa entra na reconstrução).
fn read_saved(path: &Path) -> ScreenLayout {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ScreenLayout>(&bytes).ok())
        .unwrap_or(ScreenLayout { monitores: Vec::new() })
}

fn save_layout(path: &Path, layout: &ScreenLayout) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(layout).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body)?;
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fingerprint: &str, name: &str) -> PeerRecord {
        PeerRecord { fingerprint: fingerprint.into(), name: name.into() }
    }

    #[test]
    fn identidade_vem_do_pareamento_nao_do_hello() {
        let peers = [record("fp-win2", "win2"), record("fp-mac", "mac")];
        assert_eq!(bound_name(&peers, "fp-win2", "win2"), Some("win2".into()));
        assert_eq!(bound_name(&peers, "fp-win2", "mac"), Some("win2".into()), "hello mentiroso não troca a identidade");
        assert_eq!(bound_name(&peers, "fp-desconhecido", "win2"), None);
    }

    #[test]
    fn nome_novo_nao_colide_com_pareado() {
        let peers = [record("fp-win2", "win2"), record("fp-x", "win2-2")];
        assert_eq!(unique_name(&peers, "win2", "DESKTOP"), "win2-3");
        assert_eq!(unique_name(&peers, "mac", "DESKTOP"), "mac");
        assert_eq!(unique_name(&peers, "DESKTOP", "DESKTOP"), "DESKTOP-2", "nome da casa é reservado");
    }

    #[test]
    fn so_bloqueio_pedido_pelo_usuario_bloqueia_as_outras() {
        assert!(user_locked(Duration::from_secs(1)), "Win+L: teclas acabaram de ser apertadas");
        assert!(!user_locked(Duration::from_secs(300)), "inatividade: usuário pode estar no teclado do Mac");
    }

    #[test]
    fn recusa_e_lembrada_por_um_tempo() {
        let start = std::time::Instant::now();
        let mut memory = Rejections::default();
        assert!(!memory.recent("fp", start));
        memory.reject("fp", start);
        assert!(memory.recent("fp", start + Duration::from_secs(60)));
        assert!(!memory.recent("fp", start + REJECTION_MEMORY + Duration::from_secs(1)));
        assert!(!memory.recent("outro", start));
    }
}
