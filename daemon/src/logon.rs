//! Cliente de login separado: nunca entra no cliente completo nem aceita streams de arquivos.
use crate::{
    config::{Config, Role},
    logon_protocol::{Input, PACKET_SIZE, helper_connects},
    logon_windows as win,
};
use mousex_core::{
    discovery,
    identity::DeviceIdentity,
    net,
    proto::{Control, MonitorInfo, Motion, is_newer, read_known, write_control},
};
use std::{error::Error, time::Duration};
use tokio::io::AsyncReadExt;

pub fn run() -> Result<(), Box<dyn Error>> {
    win::require_system()?;
    let _ = crate::platform::prepare();
    let dir = crate::config::system_data_dir().ok_or("ProgramData folder unavailable")?;
    use std::os::windows::fs::MetadataExt;
    if std::fs::symlink_metadata(&dir)?.file_attributes() & 0x400 != 0 || !mousex_native::owned_by_admins(&dir)? {
        return Err("untrusted logon configuration folder".into());
    }
    // Somente identidade já instalada. Nenhuma geração, migração ou pareamento interativo como SYSTEM.
    let identity = DeviceIdentity::load(&dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let input = win::InputWorker::new();
        let pipe_input = input.clone();
        let server = win::create_pipe()?;
        let mut pipe = tokio::spawn(async move { serve_pipe(server, pipe_input, win::next_pipe).await });
        let mut poll = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                result = &mut pipe => {
                    result??;
                    return Err::<(), Box<dyn Error>>("logon input pipe stopped".into());
                }
                _ = poll.tick() => {}
            }
            if !helper_connects(win::agent_present(), true) {
                continue;
            }
            let Some(config) = Config::load(&dir).filter(|c| c.role == Role::Client) else {
                continue;
            };
            if config.server_fingerprint.is_none() {
                continue;
            }
            let endpoint = net::client_endpoint(&identity)?;
            // Cancelar derruba o endpoint e a sessão, inclusive durante descoberta/handshake.
            tokio::select! {
                _ = network_loop(&endpoint, &config, &dir, &input) => {}
                _ = async {
                    loop {
                        poll.tick().await;
                        if !helper_connects(win::agent_present(), true) || pipe.is_finished() { break; }
                    }
                } => {}
            }
            endpoint.close(0u32.into(), b"user agent handoff");
            let _ = input.send(Input::Release);
            endpoint.wait_idle().await;
        }
    })
}

async fn serve_pipe(
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    input: impl InputSink,
    next: impl Fn() -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer>,
) -> std::io::Result<()> {
    loop {
        server.connect().await?;
        // Abre a próxima antes de fechar esta: nunca libera o nome. Um handle novo também evita
        // reutilizar o estado de leitura encerrada que o Mio mantém após desconectar um pipe.
        let waiting = next()?;
        let mut bytes = [0u8; PACKET_SIZE];
        loop {
            if server.read_exact(&mut bytes).await.is_err() {
                break;
            }
            let Some(event) = Input::decode(&bytes) else {
                break;
            };
            if input.send(event).is_err() {
                break;
            }
        }
        let _ = input.send(Input::Release);
        drop(server);
        server = waiting;
    }
}

async fn network_loop(endpoint: &quinn::Endpoint, config: &Config, dir: &std::path::Path, input: &win::InputWorker) {
    let Some(wanted) = &config.server_fingerprint else {
        return;
    };
    let cache = dir.join("last_server.json");
    let mut backoff = Duration::from_millis(250);
    loop {
        let mut wait = backoff;
        if let Some(address) = discovery::find_read_only(wanted, &cache, Duration::from_secs(2)).await
            && let Ok(mut link) = net::connect(endpoint, address).await
        {
            if link.peer_fingerprint != *wanted {
                link.conn.close(crate::client::REJECTED.into(), b"unpaired server");
            } else {
                let monitors = crate::platform::local_monitors();
                let hello = Control::Hello { machine: config.machine.clone(), monitors: monitors.clone() };
                if write_control(&mut link.control_tx, &hello).await.is_ok() {
                    let started = std::time::Instant::now();
                    if input_session(link, config.machine.clone(), monitors, input).await {
                        wait = crate::client::REJECTED_WAIT;
                    } else if started.elapsed() >= Duration::from_secs(5) {
                        backoff = Duration::from_millis(250);
                        wait = backoff;
                    }
                }
            }
            let _ = input.send(Input::Release);
        }
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(Duration::from_secs(2));
    }
}

struct Reader(tokio::task::JoinHandle<()>);
impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

trait InputSink {
    fn send(&self, input: Input) -> std::io::Result<()>;
}
impl InputSink for win::InputWorker {
    fn send(&self, input: Input) -> std::io::Result<()> {
        self.send(input)
    }
}

async fn input_session(
    link: net::Link,
    machine: String,
    mut monitors: Vec<MonitorInfo>,
    input: &impl InputSink,
) -> bool {
    let net::Link { conn, mut control_rx, mut control_tx, .. } = link;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    // Uma única leitura persistente: read_known não pode ser cancelado no meio de um frame.
    let _reader = Reader(tokio::spawn(async move {
        while let Ok(message) = read_known(&mut control_rx).await {
            if tx.send(message).await.is_err() {
                break;
            }
        }
    }));
    let mut seq = None;
    let interval = Duration::from_secs(2);
    let mut poll = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    loop {
        let event = tokio::select! {
            message = rx.recv() => match message {
                None => break,
                Some(Control::Key { hid, down }) => Some(Input::Key { hid, down }),
                Some(Control::Enter { monitor_id, x, y }) => position(&monitors, &monitor_id, x, y),
                Some(Control::Button { button, down, monitor_id, x, y }) => {
                    if let Some(motion) = position(&monitors, &monitor_id, x, y) { let _ = input.send(motion); }
                    Some(Input::Button { button, down })
                }
                Some(Control::Wheel { dx, dy }) => Some(Input::Wheel { dx: dx.clamp(-32768, 32767), dy: dy.clamp(-32768, 32767) }),
                Some(Control::Leave | Control::Lock) => Some(Input::Release),
                // Nenhum clipboard, arquivos, janela de identificação ou ação de sessão.
                Some(_) => None,
            },
            bytes = conn.read_datagram() => match bytes {
                Err(_) => break,
                Ok(bytes) => Motion::decode(&bytes).filter(|m| seq.is_none_or(|last| is_newer(m.seq, last)))
                    .and_then(|m| { seq = Some(m.seq); monitors.get(m.monitor as usize)
                        .map(|screen| absolute(screen, m.x, m.y)) }),
            },
            stream = conn.accept_uni() => {
                match stream {
                    Ok(mut stream) => { let _ = stream.stop(0u32.into()); }
                    Err(_) => break,
                }
                None
            }
            _ = poll.tick() => {
                let now = crate::platform::local_monitors();
                if !now.is_empty() && now != monitors {
                    if write_control(&mut control_tx, &Control::Hello { machine: machine.clone(), monitors: now.clone() }).await.is_err() { break; }
                    monitors = now;
                }
                None
            }
        };
        if let Some(event) = event {
            // A mesma validação estrita vale para rede e pipe, inclusive HIDs/botões desconhecidos.
            if Input::decode(&event.encode()).is_some() && input.send(event).is_err() {
                break;
            }
        }
    }
    let rejected = matches!(conn.close_reason(), Some(quinn::ConnectionError::ApplicationClosed(ref close))
        if close.error_code == quinn::VarInt::from_u32(crate::client::REJECTED));
    conn.close(0u32.into(), b"logon session ended");
    rejected
}

fn absolute(m: &MonitorInfo, x: i32, y: i32) -> Input {
    Input::Move {
        x: m.x.saturating_add(x.clamp(0, m.width.saturating_sub(1).max(0))),
        y: m.y.saturating_add(y.clamp(0, m.height.saturating_sub(1).max(0))),
    }
}
fn position(monitors: &[MonitorInfo], id: &str, x: i32, y: i32) -> Option<Input> {
    monitors.iter().find(|m| m.id == id).map(|m| absolute(m, x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Record(tokio::sync::mpsc::UnboundedSender<Input>);
    impl InputSink for Record {
        fn send(&self, event: Input) -> std::io::Result<()> {
            self.0.send(event).map_err(std::io::Error::other)
        }
    }

    fn screen() -> MonitorInfo {
        MonitorInfo { id: "d1".into(), x: -1920, y: 0, width: 1920, height: 1080, scale: 1.0 }
    }

    #[test]
    fn coordenadas_invalidas_ficam_dentro_do_monitor() {
        let monitors = [screen()];
        assert_eq!(position(&monitors, "d1", i32::MIN, i32::MAX), Some(Input::Move { x: -1920, y: 1079 }));
        assert_eq!(position(&monitors, "unknown", 0, 0), None);
    }

    #[tokio::test]
    async fn sessao_de_login_aceita_so_entrada_e_ignora_streams_de_arquivos() {
        let root = std::env::temp_dir().join(format!("mousex-logon-test-{}", rand::random::<u64>()));
        let host = DeviceIdentity::load_or_create(&root.join("host")).expect("identidade");
        let identity = DeviceIdentity::load_or_create(&root.join("client")).expect("identidade");
        let server = net::server_endpoint(&host, 0).expect("servidor");
        let endpoint = net::client_endpoint(&identity).expect("cliente");
        let address = ([127, 0, 0, 1], server.local_addr().expect("endereço").port()).into();
        let (client, peer) = tokio::join!(net::connect(&endpoint, address), net::accept(&server));
        let client = client.expect("conexão");
        let mut peer = peer.expect("conexão");
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move { input_session(client, "win2".into(), vec![screen()], &Record(tx)).await });
        let mut stream = peer.conn.open_uni().await.expect("stream");
        stream.write_all(b"\x03\x01\x00\x00\x00/").await.expect("arquivo malformado");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), stream.stopped()).await.expect("recusa").expect("stream"),
            Some(0u32.into())
        );
        for message in [
            Control::ClipboardPull,
            Control::Identify { screens: vec![("d1".into(), 1)] },
            Control::Activity,
            Control::Key { hid: 0xffff, down: true },
            Control::Button { button: 99, down: true, monitor_id: "missing".into(), x: 0, y: 0 },
            Control::Key { hid: 4, down: true },
            Control::Leave,
        ] {
            write_control(&mut peer.control_tx, &message).await.expect("controle");
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events.recv()).await.expect("tecla"),
            Some(Input::Key { hid: 4, down: true })
        );
        assert_eq!(events.recv().await, Some(Input::Release));
        assert!(events.try_recv().is_err(), "somente entrada válida foi encaminhada");
        peer.conn.close(0u32.into(), b"test ended");
        assert!(!tokio::time::timeout(Duration::from_secs(2), task).await.expect("fim").expect("task"));
        assert_eq!(std::fs::read_dir(&root).expect("pasta").count(), 2, "nenhum arquivo recebido");
        std::fs::remove_dir_all(root).expect("limpeza");
    }

    #[tokio::test]
    async fn pipe_recusa_pacote_invalido_solta_teclas_e_reconecta() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
        let name = format!(r"\\.\pipe\MouseXLogon-parser-test-{}", rand::random::<u64>());
        // Pipe temporário com ACL do usuário do teste; a ACL de produção tem teste próprio.
        let server = ServerOptions::new().first_pipe_instance(true).max_instances(2).create(&name).expect("pipe");
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let next_name = name.clone();
        let task = tokio::spawn(serve_pipe(server, Record(tx), move || {
            ServerOptions::new().max_instances(2).create(&next_name)
        }));
        let mut peer = ClientOptions::new().open(&name).expect("cliente");
        peer.write_all(&Input::Key { hid: 4, down: true }.encode()).await.expect("tecla");
        peer.write_all(&[0; PACKET_SIZE]).await.expect("inválido");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events.recv()).await.expect("tecla"),
            Some(Input::Key { hid: 4, down: true })
        );
        assert_eq!(events.recv().await, Some(Input::Release));
        drop(peer);
        let mut peer = ClientOptions::new().open(&name).expect("reconexão");
        peer.write_all(&Input::Button { button: 0, down: true }.encode()).await.expect("botão");
        // Uma mensagem incompleta seguida de EOF também libera tudo, sem injetar o fragmento.
        peer.write_all(&[1, 1]).await.expect("parcial");
        assert_eq!(events.recv().await, Some(Input::Button { button: 0, down: true }));
        drop(peer);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events.recv()).await.expect("soltura"),
            Some(Input::Release)
        );
        assert!(events.try_recv().is_err());
        task.abort();
    }

    #[tokio::test]
    async fn sessao_de_login_distingue_recusa_de_queda() {
        let root = std::env::temp_dir().join(format!("mousex-logon-rejected-{}", rand::random::<u64>()));
        let host = DeviceIdentity::load_or_create(&root.join("host")).expect("identidade");
        let identity = DeviceIdentity::load_or_create(&root.join("client")).expect("identidade");
        let server = net::server_endpoint(&host, 0).expect("servidor");
        let endpoint = net::client_endpoint(&identity).expect("cliente");
        let address = ([127, 0, 0, 1], server.local_addr().expect("endereço").port()).into();
        let (client, peer) = tokio::join!(net::connect(&endpoint, address), net::accept(&server));
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let client = client.expect("cliente");
        let task = tokio::spawn(async move { input_session(client, "win2".into(), vec![screen()], &Record(tx)).await });
        peer.expect("servidor").conn.close(crate::client::REJECTED.into(), b"pairing refused");
        assert!(tokio::time::timeout(Duration::from_secs(2), task).await.expect("fim").expect("task"));
        assert!(events.try_recv().is_err());
        std::fs::remove_dir_all(root).expect("limpeza");
    }
}
