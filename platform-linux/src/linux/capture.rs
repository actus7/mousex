//! Wayland server capture: pointer barriers on the InputCapture portal, then libei
//! (receiver) for keys, motion, buttons and the wheel while the pointer is on another computer.
//! Only edges that touch another computer's screen get a barrier; the rest stay the desktop's (a dock
//! that hides at the bottom, hot corners). GNOME 46 takes no new barriers while the session is enabled,
//! so a layout change disables it, replaces the barriers and enables it again, in the same session.
//! While the capture is active the desktop sends every key and motion here only: whatever stalls (the portal,
//! libei, the agent's loop) has a deadline, after which the keyboard and mouse go back to the desktop.
use super::other;
use ashpd::desktop::{
    PersistMode,
    input_capture::{
        Barrier, BarrierID, Capabilities, CreateSessionOptions, InputCapture, ReleaseOptions, StartOptions,
    },
};
use futures_util::StreamExt;
use reis::{
    ei,
    event::{Connection, DeviceCapability, EiEvent},
};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc as tokio_mpsc;

/// A compositor zone, in the same pixel space as barrier hits.
#[derive(Debug, Clone)]
pub struct Zone {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// What the compositor delivered. Keys are evdev codes; the daemon maps them.
#[derive(Debug)]
pub enum Captured {
    /// Pointer hit a barrier. Coordinates are one pixel past that edge, so the router can enter.
    Move {
        x: i32,
        y: i32,
    },
    Delta {
        dx: i32,
        dy: i32,
    },
    Key {
        evdev: u16,
        down: bool,
    },
    /// Protocol button: 0 left, 1 right, 2 middle, 3 back, 4 forward.
    Button {
        button: u8,
        down: bool,
    },
    /// Windows wheel delta (120 per notch, positive is up or right).
    Wheel {
        delta: i16,
        horizontal: bool,
    },
    SessionLocked,
    /// The desktop ended the capture by itself while the cursor was on another computer: go home.
    Released,
    /// Diagnostic line for the agent log. Not an input event.
    Note(String),
}

pub struct ServerCapture {
    pub events: std::sync::mpsc::Receiver<Captured>,
    pub zones: Vec<Zone>,
}

/// Another computer's screen in the layout, in the same pixel space as the zones.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

enum Cmd {
    /// The server kept the pointer: forward libei events.
    Hold,
    /// No neighbor on this edge, or the cursor came back. `None` nudges inside the barrier.
    Release(Option<(i32, i32)>),
    /// The other computers' screens moved: barriers follow them.
    Neighbors(Vec<Area>),
    /// The agent's loop is running (every few seconds).
    Alive,
}

static COMMANDS: OnceLock<tokio_mpsc::UnboundedSender<Cmd>> = OnceLock::new();
/// Set when a barrier hit is waiting for the server to hold or release it.
static PENDING: AtomicBool = AtomicBool::new(false);
/// Portal calls with the capture possibly active: past this the desktop is not answering.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(3);
/// After an edge hit the server must take it, and input must start flowing, within this time.
const ACTIVATION_GRACE: Duration = Duration::from_secs(3);
/// With the cursor on another computer, the agent's loop must show it is alive this often.
const SERVER_SILENCE: Duration = Duration::from_secs(6);
/// gnome-shell 46 crashed (SIGSEGV) when the barriers changed right after the capture was enabled.
const SETTLE: Duration = Duration::from_secs(3);

/// `neighbors`: the other computers' screens. Only edges that touch one get a barrier.
pub fn start_server_capture(token_file: PathBuf, neighbors: Vec<Area>) -> io::Result<ServerCapture> {
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel();
    COMMANDS.set(cmd_tx).map_err(|_| other("input capture already started"))?;
    std::thread::Builder::new().name("mousex-capture".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
        };
        let result = runtime.block_on(run(token_file, neighbors, events_tx, cmd_rx, ready_tx));
        if let Err(error) = result {
            eprintln!("MouseX capture ended: {error}");
        }
    })?;
    match ready_rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(zones)) => Ok(ServerCapture { events: events_rx, zones }),
        Ok(Err(error)) => Err(other(error)),
        Err(_) => Err(other("the InputCapture portal did not answer")),
    }
}

/// `true` when a barrier hit is waiting and the server may keep it.
pub fn set_holding(on: bool) -> bool {
    if !on {
        return true;
    }
    if !PENDING.load(Ordering::Acquire) {
        return false;
    }
    PENDING.store(false, Ordering::Release);
    command(Cmd::Hold);
    true
}

pub fn release_pending() {
    if PENDING.swap(false, Ordering::AcqRel) {
        command(Cmd::Release(None));
    }
}

pub fn release_at(x: i32, y: i32) {
    PENDING.store(false, Ordering::Release);
    command(Cmd::Release(Some((x, y))));
}

/// The layout changed: barriers move to the edges that touch the other computers now.
pub fn set_neighbors(neighbors: Vec<Area>) {
    command(Cmd::Neighbors(neighbors));
}

/// The agent's loop is running: without this, with the cursor away, the keyboard and mouse go back home.
pub fn server_alive() {
    command(Cmd::Alive);
}

fn command(cmd: Cmd) {
    if let Some(sender) = COMMANDS.get() {
        let _ = sender.send(cmd);
    }
}

#[derive(Clone, Copy)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone)]
struct Armed {
    edge: Edge,
    zone: Zone,
}

async fn run(
    token_file: PathBuf,
    neighbors: Vec<Area>,
    events: std::sync::mpsc::Sender<Captured>,
    mut commands: tokio_mpsc::UnboundedReceiver<Cmd>,
    ready: std::sync::mpsc::Sender<Result<Vec<Zone>, String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let portal = InputCapture::new().await.map_err(|error| error.to_string())?;
    let session = open_session(&portal, &token_file).await?;
    let zones_reply = portal.zones(&session, Default::default()).await?.response()?;
    let zones = zones_of(&zones_reply);
    if zones.is_empty() {
        let error = "the InputCapture portal reported no screens".to_string();
        let _ = ready.send(Err(error.clone()));
        return Err(error.into());
    }
    let mut wanted = wanted_barriers(&zones, &neighbors);
    // No barrier list at all when no other computer touches these screens (gnome-shell 46 crashed on an empty
    // one): the capture stays off until a neighbor shows up.
    let mut armed = if wanted.is_empty() {
        HashMap::new()
    } else {
        arm_barriers(&portal, &session, &wanted, zones_reply.zone_set()).await?
    };
    note(&events, &format!("pointer barriers on {} screen edge(s)", armed.len()));
    let fd = portal.connect_to_eis(&session, Default::default()).await?;
    let context = ei::Context::new(std::os::unix::net::UnixStream::from(fd))?;
    let (mut connection, mut ei_events) =
        context.handshake_tokio("MouseX", ei::handshake::ContextType::Receiver).await?;
    let _ = ready.send(Ok(zones.clone()));
    note(&events, "input capture session open; the desktop may ask to allow capturing the keyboard and mouse");
    if !armed.is_empty() {
        if let Err(error) = portal.enable(&session, Default::default()).await {
            note(&events, &format!("input capture was not allowed: {error}"));
            return Err(error.into());
        }
        note(&events, "input capture enabled");
    }
    let mut enabled_at = tokio::time::Instant::now();

    let mut activated_stream = portal.receive_activated().await?;
    let mut deactivated = portal.receive_deactivated().await?;
    let mut zones_changed = portal.receive_zones_changed().await?;
    let mut disabled = portal.receive_disabled().await?;
    // `gdbus` to logind can take seconds: never in the loop that forwards keys and motion.
    let lock_events = events.clone();
    tokio::spawn(async move {
        let mut locked_was = false;
        let mut lock_poll = tokio::time::interval(Duration::from_secs(1));
        loop {
            lock_poll.tick().await;
            let locked = tokio::task::spawn_blocking(session_locked).await.unwrap_or(false);
            if locked && !locked_was && lock_events.send(Captured::SessionLocked).is_err() {
                break;
            }
            locked_was = locked;
        }
    });
    let mut fraction = (0.0f32, 0.0f32);
    let mut activation: Option<u32> = None;
    let mut fallback_release: Option<(i32, i32)> = None;
    let mut holding = false;
    // New neighbors: the barriers move once the cursor is home and the capture has settled.
    let mut waiting: Option<Vec<Area>> = None;
    // Disabling to move barriers may come back as a Disabled signal: not a reason to restart.
    let mut quiet_until: Option<tokio::time::Instant> = None;
    // Edge hit not yet confirmed by input forwarded to the server, and whether input arrived since it.
    let mut activated_at: Option<tokio::time::Instant> = None;
    let mut input_seen = false;
    let mut alive_at = tokio::time::Instant::now();
    let mut check = tokio::time::interval(Duration::from_millis(500));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            Some(_) = deactivated.next() => {
                activated_at = None;
                // After our own Release `holding` and `activation` are already clear. Coming from the desktop
                // with the cursor away, the server must come home, or it keeps waiting for motion that never comes.
                if holding || activation.is_some() {
                    holding = false;
                    activation = None;
                    PENDING.store(false, Ordering::Release);
                    note(&events, "the desktop ended the capture; the cursor comes back to this computer");
                    let _ = events.send(Captured::Released);
                }
            }
            Some(changed) = zones_changed.next() => {
                if changed.zone_set().is_some_and(|id| id != zones_reply.zone_set()) {
                    restart(&events, "screens changed; restarting so pointer barriers match them");
                }
            }
            Some(_) = disabled.next() => {
                if quiet_until.is_none_or(|until| tokio::time::Instant::now() > until) {
                    restart(&events, "input capture was disabled; restarting");
                }
            }
            Some(hit) = activated_stream.next() => {
                if holding {
                    // Activated again with the cursor still on the other computer: keep forwarding. Treating it
                    // as a new edge hit stopped the forwarding, and nothing moved until the client went away.
                    activation = hit.activation_id();
                    note(&events, "input capture activated again while on the other computer; still there");
                    continue;
                }
                activation = hit.activation_id();
                activated_at = Some(tokio::time::Instant::now());
                input_seen = false;
                let cursor = hit.cursor_position().unwrap_or((0.0, 0.0));
                let armed_edge = match hit.barrier_id() {
                    Some(ashpd::desktop::input_capture::ActivatedBarrier::Barrier(id)) => armed.get(&id.get()),
                    _ => None,
                };
                let info = armed_edge.cloned().or_else(|| nearest_edge(&zones, cursor));
                let trigger = info.as_ref().map(|item| outside_point(item, cursor)).unwrap_or((cursor.0.round() as i32, cursor.1.round() as i32));
                fallback_release = info.as_ref().map(|item| inward_point(item, cursor));
                holding = false;
                PENDING.store(true, Ordering::Release);
                let _ = events.send(Captured::Move { x: trigger.0, y: trigger.1 });
            }
            Some(cmd) = commands.recv() => {
                match cmd {
                    // The desktop may have ended the capture already (Deactivated): nothing left to hold.
                    Cmd::Hold => {
                        holding = activation.is_some();
                        // Input already arrived since the hit, before the server took it: libei is delivering.
                        if holding && input_seen {
                            activated_at = None;
                        }
                    }
                    Cmd::Alive => alive_at = tokio::time::Instant::now(),
                    Cmd::Release(place) => {
                        holding = false;
                        activated_at = None;
                        let point = place.map(|point| away_from_edges(&zones, point)).or(fallback_release);
                        if let Some(id) = activation.take() {
                            if let Err(reason) = give_back(&events, &portal, &session, id, point).await {
                                restart(&events, &format!("{reason}; restarting"));
                            }
                            enabled_at = tokio::time::Instant::now();
                        }
                    }
                    Cmd::Neighbors(next) => {
                        let next_wanted = wanted_barriers(&zones, &next);
                        waiting = if same_edges(&next_wanted, &wanted) { None } else { Some(next) };
                    }
                }
            }
            _ = check.tick() => {
                let now = tokio::time::Instant::now();
                let since_hit = activated_at.map(|at| now.duration_since(at));
                let active = holding || activation.is_some();
                if let Some(reason) = stuck(since_hit, holding, active, now.duration_since(alive_at)) {
                    // Captured with nobody forwarding: the whole desktop would be without keyboard and mouse.
                    note(&events, &format!("{reason}; giving the keyboard and mouse back to this computer"));
                    holding = false;
                    activated_at = None;
                    PENDING.store(false, Ordering::Release);
                    if let Some(id) = activation.take() {
                        if let Err(reason) = give_back(&events, &portal, &session, id, fallback_release).await {
                            restart(&events, &format!("{reason}; restarting"));
                        }
                        enabled_at = tokio::time::Instant::now();
                    }
                    let _ = events.send(Captured::Released);
                } else if !holding
                    && activation.is_none()
                    && now.duration_since(enabled_at) >= SETTLE
                    && let Some(next) = waiting.take()
                {
                    let next_wanted = wanted_barriers(&zones, &next);
                    quiet_until = Some(now + Duration::from_secs(2));
                    let moved = rearm(&portal, &session, &next_wanted, zones_reply.zone_set());
                    match tokio::time::timeout(PORTAL_TIMEOUT, moved).await {
                        Ok(Ok(now_armed)) => {
                            armed = now_armed;
                            wanted = next_wanted;
                            enabled_at = tokio::time::Instant::now();
                            note(&events, &format!("pointer barriers moved: {} screen edge(s)", armed.len()));
                        }
                        Ok(Err(error)) => {
                            restart(&events, &format!("could not move the pointer barriers: {error}; restarting"));
                        }
                        Err(_) => restart(&events, "the desktop did not answer while moving the pointer barriers; restarting"),
                    }
                }
            }
            // Without libei no key or motion arrives and nothing brings it back. Locking the screen can end it.
            event = ei_events.next() => {
                match event {
                    Some(Ok(event)) => {
                        if is_input(&event) {
                            input_seen = true;
                            if holding {
                                // Input reaches the server: the edge hit is confirmed.
                                activated_at = None;
                            }
                        }
                        if let Err(error) = on_ei(&mut connection, &events, event, holding, &mut fraction) {
                            restart(&events, &format!("libei: {error}; restarting"));
                        }
                    }
                    Some(Err(error)) => restart(&events, &format!("libei: {error}; restarting")),
                    None => restart(&events, "the compositor closed the input capture connection; restarting"),
                }
            }
        }
    }
}

fn note(events: &std::sync::mpsc::Sender<Captured>, line: &str) {
    let _ = events.send(Captured::Note(line.to_string()));
}

/// systemd starts a fresh agent (Restart=always). The pause lets the line reach the log before the exit.
fn restart(events: &std::sync::mpsc::Sender<Captured>, line: &str) -> ! {
    note(events, line);
    std::thread::sleep(Duration::from_millis(200));
    std::process::exit(1);
}

/// Gives the pointer back to the desktop and enables the capture again. Each call has a deadline: a desktop that
/// does not answer would keep the keyboard and mouse captured with nobody forwarding them. On `Err` the caller
/// restarts; the session ends with the process and the desktop gets its input back.
async fn give_back(
    events: &std::sync::mpsc::Sender<Captured>,
    portal: &InputCapture,
    session: &ashpd::desktop::Session<InputCapture>,
    activation: u32,
    point: Option<(i32, i32)>,
) -> Result<(), String> {
    let mut options = ReleaseOptions::default().set_activation_id(Some(activation));
    if let Some((x, y)) = point {
        options = options.set_cursor_position(Some((f64::from(x), f64::from(y))));
    }
    match tokio::time::timeout(PORTAL_TIMEOUT, portal.release(session, options)).await {
        Err(_) => return Err("the desktop did not answer the release of the pointer".into()),
        Ok(Err(error)) => note(events, &format!("failed to release the pointer: {error}")),
        Ok(Ok(_)) => {}
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    match tokio::time::timeout(PORTAL_TIMEOUT, portal.enable(session, Default::default())).await {
        Err(_) => Err("the desktop did not answer when enabling input capture again".into()),
        Ok(Err(error)) => Err(format!("failed to re-enable input capture: {error}")),
        Ok(Ok(_)) => Ok(()),
    }
}

/// Why the keyboard and mouse must go back to the desktop now, if they must. `since_hit`: time since an edge
/// hit that input has not confirmed yet; `active`: the desktop is capturing; `since_alive`: the agent's loop.
fn stuck(since_hit: Option<Duration>, holding: bool, active: bool, since_alive: Duration) -> Option<&'static str> {
    if since_hit.is_some_and(|elapsed| elapsed >= ACTIVATION_GRACE) {
        Some(if holding { "no input arrived after the edge hit" } else { "the agent did not take the edge hit" })
    } else if active && since_alive >= SERVER_SILENCE {
        Some("the agent's loop stopped answering")
    } else {
        None
    }
}

/// Something the user did on the keyboard or mouse: proof that the capture is delivering.
fn is_input(event: &EiEvent) -> bool {
    matches!(
        event,
        EiEvent::PointerMotion(_)
            | EiEvent::PointerMotionAbsolute(_)
            | EiEvent::Button(_)
            | EiEvent::ScrollDelta(_)
            | EiEvent::ScrollDiscrete(_)
            | EiEvent::KeyboardKey(_)
    )
}

async fn open_session(
    portal: &InputCapture,
    token_file: &Path,
) -> Result<ashpd::desktop::Session<InputCapture>, Box<dyn std::error::Error>> {
    let capabilities = Capabilities::Keyboard | Capabilities::Pointer;
    match portal.create_session2(Default::default()).await {
        Ok(session) => {
            let token = std::fs::read_to_string(token_file).ok();
            let started = portal
                .start(
                    &session,
                    None,
                    StartOptions::default()
                        .set_capabilities(capabilities)
                        .set_persist_mode(Some(PersistMode::ExplicitlyRevoked))
                        .set_restore_token(token.as_deref().map(|value| value.trim().to_string())),
                )
                .await?
                .response()?;
            if let Some(token) = started.restore_token() {
                let _ = save_token(token_file, token);
            }
            Ok(session)
        }
        Err(ashpd::Error::RequiresVersion(_, _)) => {
            let (session, _) =
                portal.create_session(None, CreateSessionOptions::default().set_capabilities(capabilities)).await?;
            Ok(session)
        }
        Err(error) => Err(error.into()),
    }
}

fn save_token(path: &Path, token: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?.write_all(token.as_bytes())
}

fn zones_of(zones: &ashpd::desktop::input_capture::Zones) -> Vec<Zone> {
    zones
        .regions()
        .iter()
        .enumerate()
        .map(|(index, region)| Zone {
            id: (index + 1).to_string(),
            x: region.x_offset(),
            y: region.y_offset(),
            width: region.width() as i32,
            height: region.height() as i32,
        })
        .filter(|zone| zone.width > 0 && zone.height > 0)
        .collect()
}

/// A barrier wanted on one screen edge; the id is stable per screen and edge.
struct Wanted {
    id: u32,
    armed: Armed,
    position: (i32, i32, i32, i32),
}

/// Barriers only where another computer's screen touches the edge. Everywhere else the edge stays the
/// desktop's: a dock that hides at the bottom, hot corners and the top bar keep working.
fn wanted_barriers(zones: &[Zone], neighbors: &[Area]) -> Vec<Wanted> {
    let mut wanted = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        for (edge_index, (edge, position)) in barrier_lines(zone).into_iter().enumerate() {
            if touches(zone, edge, neighbors) {
                let id = (index * 4 + edge_index + 1) as u32;
                wanted.push(Wanted { id, armed: Armed { edge, zone: zone.clone() }, position });
            }
        }
    }
    wanted
}

fn touches(zone: &Zone, edge: Edge, neighbors: &[Area]) -> bool {
    neighbors.iter().any(|n| {
        let rows = n.y < zone.y + zone.height && zone.y < n.y + n.height;
        let cols = n.x < zone.x + zone.width && zone.x < n.x + n.width;
        match edge {
            Edge::Left => n.x + n.width == zone.x && rows,
            Edge::Right => n.x == zone.x + zone.width && rows,
            Edge::Top => n.y + n.height == zone.y && cols,
            Edge::Bottom => n.y == zone.y + zone.height && cols,
        }
    })
}

fn same_edges(a: &[Wanted], b: &[Wanted]) -> bool {
    a.iter().map(|w| w.id).eq(b.iter().map(|w| w.id))
}

async fn arm_barriers(
    portal: &InputCapture,
    session: &ashpd::desktop::Session<InputCapture>,
    wanted: &[Wanted],
    zone_set: u32,
) -> Result<HashMap<u32, Armed>, Box<dyn std::error::Error>> {
    let mut armed = HashMap::new();
    let mut barriers = Vec::new();
    for item in wanted {
        let Some(barrier_id) = BarrierID::new(item.id) else { continue };
        armed.insert(item.id, item.armed.clone());
        barriers.push(Barrier::new(barrier_id, item.position));
    }
    let failed = portal.set_pointer_barriers(session, &barriers, zone_set, Default::default()).await?.response()?;
    for id in failed.failed_barriers() {
        armed.remove(&id.get());
    }
    Ok(armed)
}

/// GNOME 46 takes no new barriers while the session is enabled: disable, set them, enable again. It is the
/// same session, so the desktop does not ask for the permission again.
async fn rearm(
    portal: &InputCapture,
    session: &ashpd::desktop::Session<InputCapture>,
    wanted: &[Wanted],
    zone_set: u32,
) -> Result<HashMap<u32, Armed>, Box<dyn std::error::Error>> {
    portal.disable(session, Default::default()).await?;
    if wanted.is_empty() {
        // No list at all (gnome-shell 46 crashed on an empty one): disabled, the capture never activates.
        return Ok(HashMap::new());
    }
    let armed = arm_barriers(portal, session, wanted, zone_set).await?;
    portal.enable(session, Default::default()).await?;
    Ok(armed)
}

/// Barrier lines as the portal spec places them: right and bottom sit one past the last pixel
/// (x + width, y + height). On the last pixel the compositor refuses them and that edge never fires.
fn barrier_lines(zone: &Zone) -> [(Edge, (i32, i32, i32, i32)); 4] {
    let (right, bottom) = (zone.x + zone.width, zone.y + zone.height);
    [
        (Edge::Left, (zone.x, zone.y, zone.x, bottom - 1)),
        (Edge::Right, (right, zone.y, right, bottom - 1)),
        (Edge::Top, (zone.x, zone.y, right - 1, zone.y)),
        (Edge::Bottom, (zone.x, bottom, right - 1, bottom)),
    ]
}

fn on_ei(
    connection: &mut Connection,
    events: &std::sync::mpsc::Sender<Captured>,
    event: EiEvent,
    holding: bool,
    fraction: &mut (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    match event {
        EiEvent::SeatAdded(added) => {
            added.seat.bind_capabilities(
                DeviceCapability::Pointer
                    | DeviceCapability::Keyboard
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll,
            );
            let _ = connection.flush();
        }
        EiEvent::PointerMotion(motion) if holding => {
            fraction.0 += motion.dx;
            fraction.1 += motion.dy;
            let dx = fraction.0.trunc() as i32;
            let dy = fraction.1.trunc() as i32;
            fraction.0 -= dx as f32;
            fraction.1 -= dy as f32;
            if (dx, dy) != (0, 0) {
                let _ = events.send(Captured::Delta { dx, dy });
            }
        }
        EiEvent::KeyboardKey(key) if holding => {
            let _ = events.send(Captured::Key {
                evdev: key.key as u16,
                down: matches!(key.state, ei::keyboard::KeyState::Press),
            });
        }
        EiEvent::Button(button) if holding => {
            let index = button.button.wrapping_sub(0x110);
            if index <= 4 {
                let _ = events.send(Captured::Button {
                    button: index as u8,
                    down: matches!(button.state, ei::button::ButtonState::Press),
                });
            }
        }
        EiEvent::ScrollDiscrete(scroll) if holding => {
            if scroll.discrete_dy != 0 {
                let _ = events.send(Captured::Wheel { delta: wheel_delta(-scroll.discrete_dy), horizontal: false });
            }
            if scroll.discrete_dx != 0 {
                let _ = events.send(Captured::Wheel { delta: wheel_delta(scroll.discrete_dx), horizontal: true });
            }
        }
        EiEvent::Disconnected(_) => return Err("libei disconnected".into()),
        _ => {}
    }
    Ok(())
}

/// libei discrete scroll is either one notch or a multiple of 120.
fn wheel_delta(value: i32) -> i16 {
    let scaled = if value.abs() <= 16 { value.saturating_mul(120) } else { value };
    scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// logind's LockedHint. `gdbus` can take seconds: never in a loop that forwards input.
pub fn session_locked() -> bool {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--system",
            "--dest",
            "org.freedesktop.login1",
            "--object-path",
            "/org/freedesktop/login1/session/auto",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.freedesktop.login1.Session",
            "LockedHint",
        ])
        .output();
    output.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("true"))
}

fn outside_point(armed: &Armed, cursor: (f32, f32)) -> (i32, i32) {
    let zone = &armed.zone;
    let x = (cursor.0.round() as i32).clamp(zone.x, zone.x + zone.width - 1);
    let y = (cursor.1.round() as i32).clamp(zone.y, zone.y + zone.height - 1);
    match armed.edge {
        Edge::Left => (zone.x - 1, y),
        Edge::Right => (zone.x + zone.width, y),
        Edge::Top => (x, zone.y - 1),
        Edge::Bottom => (x, zone.y + zone.height),
    }
}

/// Volta do outro computador: o cursor fica a uns pixels da barreira. Rente a ela, um tremor da mão para o
/// lado de lá já levava o cursor de volta ao outro computador.
const RETURN_MARGIN: i32 = 20;

fn away_from_edges(zones: &[Zone], (x, y): (i32, i32)) -> (i32, i32) {
    let Some(zone) = zones.iter().find(|z| x >= z.x && x < z.x + z.width && y >= z.y && y < z.y + z.height) else {
        return (x, y);
    };
    let margin = RETURN_MARGIN.min(zone.width / 4).min(zone.height / 4);
    (
        x.clamp(zone.x + margin, zone.x + zone.width - 1 - margin),
        y.clamp(zone.y + margin, zone.y + zone.height - 1 - margin),
    )
}

fn inward_point(armed: &Armed, cursor: (f32, f32)) -> (i32, i32) {
    let zone = &armed.zone;
    let mut x = cursor.0.round() as i32;
    let mut y = cursor.1.round() as i32;
    match armed.edge {
        Edge::Left => x = zone.x + 2,
        Edge::Right => x = zone.x + zone.width - 3,
        Edge::Top => y = zone.y + 2,
        Edge::Bottom => y = zone.y + zone.height - 3,
    }
    (x.clamp(zone.x, zone.x + zone.width - 1), y.clamp(zone.y, zone.y + zone.height - 1))
}

fn nearest_edge(zones: &[Zone], cursor: (f32, f32)) -> Option<Armed> {
    let (x, y) = (cursor.0, cursor.1);
    zones
        .iter()
        .flat_map(|zone| {
            let right = zone.x as f32 + zone.width as f32;
            let bottom = zone.y as f32 + zone.height as f32;
            [
                (Edge::Left, (x - zone.x as f32).abs()),
                (Edge::Right, (right - x).abs()),
                (Edge::Top, (y - zone.y as f32).abs()),
                (Edge::Bottom, (bottom - y).abs()),
            ]
            .into_iter()
            .map(|(edge, distance)| (distance, Armed { edge, zone: zone.clone() }))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, armed)| armed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Armed {
        Armed { edge: Edge::Right, zone: Zone { id: "1".into(), x: 0, y: 0, width: 1920, height: 1080 } }
    }

    #[test]
    fn borda_direita_sai_um_pixel_e_a_volta_entra() {
        let armed = screen();
        assert_eq!(outside_point(&armed, (1919.0, 400.0)), (1920, 400));
        assert_eq!(inward_point(&armed, (1919.0, 400.0)), (1917, 400));
    }

    #[test]
    fn captura_sem_ninguem_repassando_devolve_teclado_e_mouse() {
        let secs = Duration::from_secs;
        assert_eq!(stuck(Some(secs(1)), false, true, secs(0)), None, "o servidor ainda pode assumir");
        assert_eq!(stuck(Some(secs(3)), false, true, secs(0)), Some("the agent did not take the edge hit"));
        assert_eq!(stuck(Some(secs(3)), true, true, secs(0)), Some("no input arrived after the edge hit"));
        assert_eq!(stuck(None, true, true, secs(5)), None, "entrada confirmada e loop batendo");
        assert_eq!(stuck(None, true, true, secs(6)), Some("the agent's loop stopped answering"));
        assert_eq!(stuck(None, false, false, secs(600)), None, "cursor em casa: nada preso");
    }

    #[test]
    fn volta_do_outro_computador_fica_longe_da_barreira() {
        let zones = [screen().zone, Zone { id: "2".into(), x: 1920, y: 120, width: 1920, height: 1080 }];
        assert_eq!(away_from_edges(&zones, (3838, 600)), (3819, 600));
        assert_eq!(away_from_edges(&zones, (3838, 125)), (3819, 140));
        assert_eq!(away_from_edges(&zones, (900, 500)), (900, 500));
    }

    #[test]
    fn so_a_borda_que_encosta_noutro_computador_ganha_barreira() {
        // Duas telas da casa; a de baixo de cada uma fica livre para o dock escondido.
        let zones = [
            Zone { id: "1".into(), x: 0, y: 0, width: 1920, height: 1200 },
            Zone { id: "2".into(), x: 1920, y: 120, width: 1920, height: 1080 },
        ];
        let ids = |neighbors: &[Area]| wanted_barriers(&zones, neighbors).iter().map(|w| w.id).collect::<Vec<_>>();
        let mac_left = Area { x: -2056, y: 0, width: 2056, height: 1329 };
        let mac_right = Area { x: 3840, y: 120, width: 2056, height: 1329 };
        assert_eq!(ids(&[mac_left]), [1]);
        assert_eq!(ids(&[mac_right]), [6]);
        assert!(ids(&[]).is_empty());
    }

    #[test]
    fn barreira_da_direita_e_de_baixo_fica_um_pixel_depois_da_tela() {
        // Exemplo da especificação do portal: tela 1920x1080 em 1920,0.
        let zone = Zone { id: "2".into(), x: 1920, y: 0, width: 1920, height: 1080 };
        assert_eq!(
            barrier_lines(&zone).map(|(_, line)| line),
            [(1920, 0, 1920, 1079), (3840, 0, 3840, 1079), (1920, 0, 3839, 0), (1920, 1080, 3839, 1080)]
        );
    }

    #[test]
    fn um_dente_de_roda_vira_120() {
        assert_eq!(wheel_delta(1), 120);
        assert_eq!(wheel_delta(-120), -120);
    }
}
