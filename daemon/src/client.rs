//! Papel cliente: localiza o servidor, conecta, injeta e reconecta para sempre.
use crate::clip;
use crate::clip::Clip;
use crate::link_status;
use mousex_core::{
    net::{self, Link},
    proto::{Control, MonitorInfo, Motion, is_newer, read_known, write_control},
};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::mpsc;

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(2);
/// Só uma sessão que durou isso zera o backoff: conectar e cair na hora não vira laço apertado.
const STABLE_SESSION: Duration = Duration::from_secs(5);
/// Intervalo para notar telas rearranjadas, plugadas ou desplugadas com o serviço rodando.
const MONITOR_POLL: Duration = Duration::from_secs(2);
/// Depois de um pareamento recusado, espera antes de incomodar o servidor de novo.
const REJECTED_WAIT: Duration = Duration::from_secs(30);
/// Código de fechamento QUIC do servidor para "pareamento recusado".
pub const REJECTED: u32 = 2;

/// Entrada e clipboard da máquina local; a implementação real fica em `platform.rs`.
pub trait Platform: Send {
    fn key(&mut self, hid: u16, down: bool);
    /// `monitor` é o índice em `Hello.monitors`; `x`/`y` são coordenadas locais desse monitor.
    fn move_to(&mut self, monitor: usize, x: i32, y: i32);
    fn button(&mut self, button: u8, down: bool);
    fn wheel(&mut self, dx: i32, dy: i32);
    fn release_all(&mut self);
    fn clipboard_seq(&self) -> u32;
    fn clipboard_read(&self) -> Option<Clip>;
    fn clipboard_write(&mut self, clip: Clip);
    /// Usuário ativo em outra máquina: não bloquear a tela por inatividade.
    fn keep_awake(&mut self);
    /// Bloquear a tela agora (o servidor foi bloqueado).
    fn lock_screen(&mut self);
    /// Monitores como estão agora (o usuário pode rearranjar as telas com o serviço rodando).
    fn monitors(&self) -> Vec<MonitorInfo>;
    /// Origens usadas por `move_to`, na ordem do último Hello enviado.
    fn set_monitors(&mut self, monitors: &[MonitorInfo]);
    /// "Identificar telas": número em cada monitor (id nativo → número) por alguns segundos.
    fn identify(&mut self, screens: &[(String, u32)]);
}

/// Monitores locais que recebem número: cruza os ids nativos da mensagem com os monitores atuais.
pub fn numbered(monitors: &[MonitorInfo], screens: &[(String, u32)]) -> Vec<(MonitorInfo, u32)> {
    monitors
        .iter()
        .filter_map(|m| screens.iter().find(|(id, _)| *id == m.id).map(|(_, number)| (m.clone(), *number)))
        .collect()
}

/// Nunca retorna: procura o servidor, conecta e, a cada queda, solta tudo e tenta de novo.
// Fn + Future explícitos (não AsyncFn): futures de closures async emprestam a closure e perdem o Send.
pub async fn client_loop<P, L, LF, A, AF>(
    endpoint: quinn::Endpoint,
    locate: L,
    authorize: A,
    hello: impl Fn() -> Control,
    mut platform: P,
) where
    P: Platform,
    L: Fn() -> LF,
    LF: Future<Output = Option<SocketAddr>>,
    A: Fn(String, [u8; 32]) -> AF,
    AF: Future<Output = bool>,
{
    let mut backoff = BACKOFF_START;
    loop {
        crate::activity::set("looking", "");
        let mut wait = backoff;
        if let Some(address) = locate().await
            && let Ok(mut link) = net::connect(&endpoint, address).await
        {
            crate::activity::set("connected", &address.to_string());
            let message = hello();
            // Hello antes da autorização: o servidor mostra o nome da máquina no diálogo de pareamento.
            if mousex_core::proto::write_control(&mut link.control_tx, &message).await.is_ok()
                && authorize(link.peer_fingerprint.clone(), net::pairing_secret(&link.conn)).await
            {
                let started = std::time::Instant::now();
                let rejected = session(link, &message, &mut platform).await;
                platform.release_all();
                crate::activity::set(if rejected { "rejected" } else { "looking" }, "");
                link_status::set(link_status::Link::Searching);
                if rejected {
                    wait = REJECTED_WAIT;
                } else if started.elapsed() >= STABLE_SESSION {
                    backoff = BACKOFF_START;
                    wait = BACKOFF_START;
                }
            } else {
                crate::activity::set("rejected", "");
                link.conn.close(REJECTED.into(), b"recusado");
            }
        }
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Retorna `true` se o servidor encerrou por recusa de pareamento.
async fn session<P: Platform>(link: Link, hello: &Control, platform: &mut P) -> bool {
    let Link { conn, mut control_tx, mut control_rx, .. } = link;
    let (machine, mut current) = match hello {
        Control::Hello { machine, monitors } => (machine.clone(), monitors.clone()),
        _ => (String::new(), Vec::new()),
    };
    platform.set_monitors(&current);
    link_status::set(link_status::Link::Connected(String::new()));
    let index = |current: &[MonitorInfo], id: &str| current.iter().position(|m| m.id == id);
    let mut poll = tokio::time::interval_at(tokio::time::Instant::now() + MONITOR_POLL, MONITOR_POLL);

    // Leituras de stream não são cancel-safe dentro de select!: cada fonte tem sua task e seu canal.
    let (controls_tx, mut controls) = mpsc::channel(256);
    let reader = tokio::spawn(async move {
        while let Ok(message) = read_known(&mut control_rx).await {
            if controls_tx.send(message).await.is_err() {
                break;
            }
        }
    });
    let (clips_tx, mut clips) = mpsc::channel(4);
    let incoming = conn.clone();
    let clip_reader = tokio::spawn(async move {
        while let Ok(stream) = incoming.accept_uni().await {
            let clips_tx = clips_tx.clone();
            tokio::spawn(async move {
                if let Some(clip) = clip::receive(stream, &clip::inbox()).await {
                    let _ = clips_tx.send(clip).await;
                }
            });
        }
    });

    let mut last_seq: Option<u32> = None;
    let mut synced = platform.clipboard_seq();
    loop {
        tokio::select! {
            message = controls.recv() => match message {
                None => break,
                Some(Control::Enter { monitor_id, x, y }) => {
                    if let Some(i) = index(&current, &monitor_id) { platform.move_to(i, x, y); }
                }
                Some(Control::Key { hid, down }) => platform.key(hid, down),
                Some(Control::Button { button, down, monitor_id, x, y }) => {
                    if let Some(i) = index(&current, &monitor_id) { platform.move_to(i, x, y); }
                    platform.button(button, down);
                }
                Some(Control::Wheel { dx, dy }) => platform.wheel(dx, dy),
                Some(Control::Leave) => platform.release_all(),
                Some(Control::ClipboardPull) => {
                    let current = platform.clipboard_seq();
                    if current != synced {
                        synced = current;
                        if let Some(clip) = platform.clipboard_read() {
                            let conn = conn.clone();
                            tokio::spawn(async move { clip::send(&conn, &clip).await });
                        }
                    }
                }
                Some(Control::Activity) => platform.keep_awake(),
                Some(Control::Lock) => {
                    platform.release_all();
                    platform.lock_screen();
                }
                Some(Control::Identify { screens }) => platform.identify(&screens),
                Some(Control::Hello { .. }) => {}
                Some(Control::Server { machine }) => link_status::set(link_status::Link::Connected(machine)),
            },
            datagram = conn.read_datagram() => match datagram {
                Err(_) => break,
                Ok(bytes) => {
                    if let Some(motion) = Motion::decode(&bytes)
                        && last_seq.is_none_or(|last| is_newer(motion.seq, last))
                    {
                        last_seq = Some(motion.seq);
                        platform.move_to(motion.monitor as usize, motion.x, motion.y);
                    }
                }
            },
            _ = poll.tick() => {
                // Telas rearranjadas: origens novas aqui e Hello novo para o servidor refazer as bordas.
                // Lista vazia é estado passageiro durante a reconfiguração do sistema.
                let now = platform.monitors();
                if !now.is_empty() && now != current {
                    if write_control(&mut control_tx, &Control::Hello { machine: machine.clone(), monitors: now.clone() }).await.is_err() { break; }
                    platform.set_monitors(&now);
                    current = now;
                }
            }
            Some(clip) = clips.recv() => {
                // ponytail: gravação síncrona no loop; mover para spawn_blocking se imagens grandes engasgarem o cursor.
                platform.clipboard_write(clip);
                synced = platform.clipboard_seq();
            }
        }
    }
    reader.abort();
    clip_reader.abort();
    let rejected = matches!(conn.close_reason(), Some(quinn::ConnectionError::ApplicationClosed(ref close)) if close.error_code == quinn::VarInt::from_u32(REJECTED));
    conn.close(0u32.into(), b"fim");
    rejected
}

#[cfg(test)]
mod tests {
    use super::*;
    use mousex_core::{
        identity::DeviceIdentity,
        net,
        proto::{MonitorInfo, Motion, read_control, write_control},
    };
    use std::{
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::Duration,
    };

    #[derive(Clone, Default)]
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        /// Monitores "reais" da máquina de teste (o teste os rearranja).
        screens: Arc<Mutex<Vec<MonitorInfo>>>,
    }
    impl Fake {
        /// Entradas injetadas, sem os ajustes de monitores (feitos no início de toda sessão).
        fn log(&self) -> Vec<String> {
            self.log.lock().expect("log").iter().filter(|l| !l.starts_with("monitores")).cloned().collect()
        }
        fn all(&self) -> Vec<String> {
            self.log.lock().expect("log").clone()
        }
        fn push(&self, line: String) {
            self.log.lock().expect("log").push(line);
        }
    }
    impl Platform for Fake {
        fn key(&mut self, hid: u16, down: bool) {
            self.push(format!("key {hid} {down}"));
        }
        fn move_to(&mut self, monitor: usize, x: i32, y: i32) {
            self.push(format!("move {monitor} {x} {y}"));
        }
        fn button(&mut self, button: u8, down: bool) {
            self.push(format!("button {button} {down}"));
        }
        fn wheel(&mut self, dx: i32, dy: i32) {
            self.push(format!("wheel {dx} {dy}"));
        }
        fn release_all(&mut self) {
            self.push("release".into());
        }
        fn clipboard_seq(&self) -> u32 {
            0
        }
        fn clipboard_read(&self) -> Option<Clip> {
            None
        }
        fn clipboard_write(&mut self, _: Clip) {}
        fn keep_awake(&mut self) {
            self.push("awake".into());
        }
        fn lock_screen(&mut self) {
            self.push("lock".into());
        }
        fn monitors(&self) -> Vec<MonitorInfo> {
            self.screens.lock().expect("telas").clone()
        }
        fn set_monitors(&mut self, monitors: &[MonitorInfo]) {
            self.push(format!("monitores {}", monitors[0].y));
        }
        fn identify(&mut self, screens: &[(String, u32)]) {
            self.push(format!("identify {}", screens.len()));
        }
    }

    fn hello() -> Control {
        Control::Hello {
            machine: "win2".into(),
            monitors: vec![MonitorInfo { id: "d1".into(), x: 0, y: 0, width: 1920, height: 1080, scale: 1.0 }],
        }
    }

    async fn wait_for(fake: &Fake, line: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !fake.all().iter().any(|l| l == line) {
            assert!(tokio::time::Instant::now() < deadline, "não chegou {line:?}; log: {:?}", fake.all());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Servidor de teste + cliente rodando `client_loop` contra ele.
    async fn setup() -> (quinn::Endpoint, Fake) {
        let root = std::env::temp_dir().join(format!("mousex-client-{}", rand::random::<u64>()));
        let host = DeviceIdentity::load_or_create(&root.join("host")).expect("host");
        let client = DeviceIdentity::load_or_create(&root.join("client")).expect("client");
        let server = net::server_endpoint(&host, 0).expect("server");
        let address: SocketAddr = ([127, 0, 0, 1], server.local_addr().expect("addr").port()).into();
        let endpoint = net::client_endpoint(&client).expect("client");
        let fake = Fake::default();
        if let Control::Hello { monitors, .. } = hello() {
            *fake.screens.lock().expect("telas") = monitors;
        }
        let platform = fake.clone();
        tokio::spawn(async move {
            client_loop(endpoint, move || async move { Some(address) }, |_, _| async { true }, hello, platform).await;
        });
        (server, fake)
    }

    async fn accept_hello(server: &quinn::Endpoint) -> net::Link {
        let mut link = net::accept(server).await.expect("accept");
        assert!(matches!(read_control(&mut link.control_rx).await.expect("hello"), Control::Hello { .. }));
        link
    }

    #[tokio::test]
    async fn cliente_recebe_e_injeta_tecla() {
        let (server, fake) = setup().await;
        let mut link = accept_hello(&server).await;
        write_control(&mut link.control_tx, &Control::Enter { monitor_id: "d1".into(), x: 5, y: 6 })
            .await
            .expect("enter");
        write_control(&mut link.control_tx, &Control::Key { hid: 4, down: true }).await.expect("key");
        write_control(
            &mut link.control_tx,
            &Control::Button { button: 0, down: true, monitor_id: "d1".into(), x: 7, y: 8 },
        )
        .await
        .expect("button");
        wait_for(&fake, "button 0 true").await;
        let log = fake.log();
        assert_eq!(&log[..4], ["move 0 5 6", "key 4 true", "move 0 7 8", "button 0 true"]);
    }

    #[tokio::test]
    async fn atividade_mantem_acordado_e_bloqueio_solta_e_bloqueia() {
        let (server, fake) = setup().await;
        let mut link = accept_hello(&server).await;
        write_control(&mut link.control_tx, &Control::Activity).await.expect("atividade");
        write_control(&mut link.control_tx, &Control::Key { hid: 4, down: true }).await.expect("key");
        write_control(&mut link.control_tx, &Control::Lock).await.expect("bloqueio");
        wait_for(&fake, "lock").await;
        let log = fake.log();
        assert_eq!(&log[..4], ["awake", "key 4 true", "release", "lock"], "teclas soltas antes de bloquear");
    }

    #[tokio::test]
    async fn telas_rearranjadas_viram_hello_novo_e_origens_novas() {
        let (server, fake) = setup().await;
        let mut link = accept_hello(&server).await;
        let moved = vec![MonitorInfo { id: "d1".into(), x: 0, y: -1080, width: 1920, height: 1080, scale: 1.0 }];
        *fake.screens.lock().expect("telas") = moved.clone();
        let message = tokio::time::timeout(Duration::from_secs(5), read_control(&mut link.control_rx))
            .await
            .expect("sem hello novo")
            .expect("leitura");
        assert_eq!(message, Control::Hello { machine: "win2".into(), monitors: moved });
        wait_for(&fake, "monitores -1080").await;
    }

    #[tokio::test]
    async fn identificar_chega_ao_cliente() {
        let (server, fake) = setup().await;
        let mut link = accept_hello(&server).await;
        let screens = vec![("d1".to_string(), 1), ("d2".to_string(), 2)];
        write_control(&mut link.control_tx, &Control::Identify { screens }).await.expect("identify");
        wait_for(&fake, "identify 2").await;
    }

    #[test]
    fn numero_so_vai_para_monitor_presente() {
        let at = |id: &str, x: i32| MonitorInfo { id: id.into(), x, y: 0, width: 1920, height: 1080, scale: 1.0 };
        let monitors = [at("d1", 0), at("d3", 1920)];
        let screens = [("d1".to_string(), 1), ("d2".to_string(), 2)];
        let found = numbered(&monitors, &screens);
        assert_eq!(found.len(), 1, "d2 não está conectada aqui");
        assert_eq!((found[0].0.id.as_str(), found[0].1), ("d1", 1));
    }

    #[tokio::test]
    async fn datagrama_antigo_e_descartado() {
        let (server, fake) = setup().await;
        let link = accept_hello(&server).await;
        for (seq, x) in [(5, 10), (3, 99), (6, 20)] {
            link.conn.send_datagram(Motion { seq, monitor: 0, x, y: 0 }.encode().to_vec().into()).expect("datagrama");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        wait_for(&fake, "move 0 20 0").await;
        assert!(!fake.log().iter().any(|l| l == "move 0 99 0"));
    }

    #[tokio::test]
    async fn desconexao_libera_injetado() {
        let (server, fake) = setup().await;
        let mut link = accept_hello(&server).await;
        write_control(&mut link.control_tx, &Control::Key { hid: 4, down: true }).await.expect("key");
        wait_for(&fake, "key 4 true").await;
        let start = std::time::Instant::now();
        drop(link);
        server.close(0u32.into(), b"teste");
        wait_for(&fake, "release").await;
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn reconecta_depois_de_queda() {
        let (server, fake) = setup().await;
        let link = accept_hello(&server).await;
        link.conn.close(0u32.into(), b"queda");
        let start = std::time::Instant::now();
        let _again =
            tokio::time::timeout(Duration::from_secs(5), accept_hello(&server)).await.expect("não reconectou em 5 s");
        assert!(start.elapsed() < Duration::from_secs(5));
        wait_for(&fake, "release").await;
    }

    #[tokio::test]
    async fn recusa_espera_antes_de_tentar_de_novo() {
        let (server, _fake) = setup().await;
        let link = accept_hello(&server).await;
        link.conn.close(REJECTED.into(), b"recusado");
        assert!(
            tokio::time::timeout(Duration::from_secs(2), accept_hello(&server)).await.is_err(),
            "reconectou logo após recusa"
        );
    }

    #[tokio::test]
    async fn sessao_curta_nao_zera_backoff() {
        let (server, _fake) = setup().await;
        let mut gaps = Vec::new();
        let mut last = std::time::Instant::now();
        for _ in 0..4 {
            let link = accept_hello(&server).await;
            gaps.push(last.elapsed());
            link.conn.close(0u32.into(), b"curta");
            last = std::time::Instant::now();
        }
        assert!(gaps[3] > gaps[1], "backoff não cresceu: {gaps:?}");
    }
}
