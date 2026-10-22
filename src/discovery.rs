//! Descoberta sem IP: mDNS/DNS-SD + broadcast UDP, identificando o servidor pelo fingerprint.
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};
use tokio::net::UdpSocket;

pub const SERVICE: &str = "_mousex._udp.local.";
pub const BROADCAST_PORT: u16 = 41415;
const QUERY: &str = "MOUSEX?";
const REPLY: &str = "MOUSEX!";
/// Fingerprint curinga: primeiro pareamento, quando o cliente ainda não conhece o servidor.
pub const ANY: &str = "*";
/// Consultas menores que isso são ignoradas: a resposta nunca é maior que a pergunta (sem amplificação).
const MIN_QUERY_BYTES: usize = 128;

/// Só redes privadas, link-local e loopback recebem resposta.
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unicast_link_local(),
    }
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// Anuncia o servidor; Drop desregistra. Sem timers próprios: só reage a pedidos.
pub struct Advertiser {
    mdns: mdns_sd::ServiceDaemon,
    responder: tokio::task::JoinHandle<()>,
}

impl Advertiser {
    pub fn start(fingerprint: &str, machine: &str, quic_port: u16) -> io::Result<Self> {
        let responder = respond_broadcast(fingerprint.into(), quic_port, BROADCAST_PORT)?;
        let mdns = mdns_sd::ServiceDaemon::new().map_err(other)?;
        let host = format!("{}.local.", machine.replace(' ', "-"));
        let properties = [("fp", fingerprint)];
        let info = mdns_sd::ServiceInfo::new(SERVICE, machine, &host, "", quic_port, &properties[..])
            .map_err(other)?
            .enable_addr_auto();
        mdns.register(info).map_err(other)?;
        Ok(Self { mdns, responder })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        self.responder.abort();
        let _ = self.mdns.shutdown();
    }
}

/// Responde `MOUSEX?<fp>` com `MOUSEX!<fp> <porta>` apenas para o próprio fingerprint.
pub(crate) fn respond_broadcast(
    fingerprint: String,
    quic_port: u16,
    broadcast_port: u16,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, broadcast_port))?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;
    Ok(tokio::spawn(async move {
        let mut buffer = [0u8; 512];
        let reply = format!("{REPLY}{fingerprint} {quic_port}");
        loop {
            let (length, from) = match socket.recv_from(&mut buffer).await {
                Ok(received) => received,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            };
            if length < MIN_QUERY_BYTES || !is_local(from.ip()) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&buffer[..length]) else { continue };
            if matches!(text.trim_end().strip_prefix(QUERY), Some(wanted) if wanted == fingerprint || wanted == ANY) {
                let _ = socket.send_to(reply.as_bytes(), from).await;
            }
        }
    }))
}

/// Endereço do servidor com `fingerprint`; descoberta tem prioridade, cache é o último recurso.
pub async fn find(fingerprint: &str, cache_path: &Path, timeout: Duration) -> Option<SocketAddr> {
    find_with(fingerprint, cache_path, timeout, BROADCAST_PORT, true).await
}

pub(crate) async fn find_with(
    fingerprint: &str,
    cache_path: &Path,
    timeout: Duration,
    broadcast_port: u16,
    use_mdns: bool,
) -> Option<SocketAddr> {
    let discovered = tokio::time::timeout(timeout, async {
        tokio::select! {
            Some(address) = query_broadcast(fingerprint, broadcast_port) => Some(address),
            Some(address) = browse_mdns(fingerprint), if use_mdns => Some(address),
            else => None,
        }
    })
    .await
    .ok()
    .flatten();
    match discovered {
        Some(address) => {
            if fingerprint != ANY {
                let _ = write_cache(cache_path, fingerprint, address);
            }
            Some(address)
        }
        None => read_cache(cache_path, fingerprint),
    }
}

async fn query_broadcast(fingerprint: &str, broadcast_port: u16) -> Option<SocketAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok()?;
    socket.set_broadcast(true).ok()?;
    let query = format!("{QUERY}{fingerprint:<width$}", width = MIN_QUERY_BYTES - QUERY.len());
    // ponytail: loopback cobre servidor e cliente na mesma máquina (testes); rede usa o broadcast.
    for target in [Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST] {
        let _ = socket.send_to(query.as_bytes(), (target, broadcast_port)).await;
    }
    let mut buffer = [0u8; 256];
    loop {
        let (length, from) = match socket.recv_from(&mut buffer).await {
            Ok(received) => received,
            // Windows entrega o ICMP "porta inalcançável" do envio ao loopback como erro de leitura.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return None,
        };
        let Ok(text) = std::str::from_utf8(&buffer[..length]) else { continue };
        let Some((found, port)) = text.strip_prefix(REPLY).and_then(|rest| rest.split_once(' ')) else { continue };
        if (found == fingerprint || fingerprint == ANY)
            && let Ok(port) = port.parse::<u16>()
        {
            return Some(SocketAddr::new(from.ip(), port));
        }
    }
}

/// Encerra o daemon mDNS mesmo quando a busca é cancelada no meio (select!/timeout).
struct MdnsGuard(mdns_sd::ServiceDaemon);

impl Drop for MdnsGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

async fn browse_mdns(fingerprint: &str) -> Option<SocketAddr> {
    let mdns = MdnsGuard(mdns_sd::ServiceDaemon::new().ok()?);
    let receiver = mdns.0.browse(SERVICE).ok()?;
    let mut found = None;
    while let Ok(event) = receiver.recv_async().await {
        let mdns_sd::ServiceEvent::ServiceResolved(info) = event else { continue };
        if fingerprint != ANY && info.get_property_val_str("fp") != Some(fingerprint) {
            continue;
        }
        // IPv4 primeiro: link-local IPv6 exige escopo de interface.
        let mut addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
        addresses.sort_by_key(|ip| !ip.is_ipv4());
        if let Some(ip) = addresses.first() {
            found = Some(SocketAddr::new(*ip, info.get_port()));
            break;
        }
    }
    found
}

#[derive(Serialize, Deserialize)]
struct CachedServer {
    fingerprint: String,
    address: SocketAddr,
}

pub(crate) fn write_cache(path: &Path, fingerprint: &str, address: SocketAddr) -> io::Result<()> {
    let body = serde_json::to_vec(&CachedServer { fingerprint: fingerprint.into(), address }).map_err(other)?;
    std::fs::write(path, body)
}

pub(crate) fn read_cache(path: &Path, fingerprint: &str) -> Option<SocketAddr> {
    let cached: CachedServer = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (cached.fingerprint == fingerprint).then_some(cached.address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mousex-cache-{}.json", rand::random::<u64>()))
    }

    #[tokio::test]
    async fn broadcast_responde_so_ao_fingerprint_certo() {
        let port = 42000 + (rand::random::<u16>() % 1000);
        let _responder = respond_broadcast("aaa".into(), 41414, port).expect("responder");
        let cache = temp_cache();
        assert_eq!(find_with("bbb", &cache, Duration::from_millis(500), port, false).await, None);
        let found = find_with("aaa", &cache, Duration::from_millis(500), port, false).await.expect("encontrado");
        assert_eq!(found.port(), 41414);
        std::fs::remove_file(cache).ok();
    }

    #[tokio::test]
    async fn cache_de_endereco_e_substituido_quando_descoberta_responde() {
        let port = 43000 + (rand::random::<u16>() % 1000);
        let cache = temp_cache();
        write_cache(&cache, "aaa", "10.255.255.1:9".parse().expect("addr")).expect("cache");
        let _responder = respond_broadcast("aaa".into(), 41414, port).expect("responder");
        let found = find_with("aaa", &cache, Duration::from_millis(500), port, false).await.expect("encontrado");
        assert_ne!(found, "10.255.255.1:9".parse::<SocketAddr>().expect("addr"));
        assert_eq!(read_cache(&cache, "aaa"), Some(found));
        std::fs::remove_file(cache).ok();
    }

    #[tokio::test]
    async fn sem_resposta_usa_cache() {
        let cache = temp_cache();
        let cached: SocketAddr = "192.168.0.9:41414".parse().expect("addr");
        write_cache(&cache, "aaa", cached).expect("cache");
        assert_eq!(find_with("aaa", &cache, Duration::from_millis(200), 44999, false).await, Some(cached));
        assert_eq!(read_cache(&cache, "outro"), None);
        std::fs::remove_file(cache).ok();
    }

    #[tokio::test]
    async fn curinga_encontra_qualquer_servidor_sem_gravar_cache() {
        let port = 45000 + (rand::random::<u16>() % 1000);
        let _responder = respond_broadcast("aaa".into(), 41414, port).expect("responder");
        let cache = temp_cache();
        let found = find_with(ANY, &cache, Duration::from_millis(500), port, false).await.expect("encontrado");
        assert_eq!(found.port(), 41414);
        assert!(!cache.exists());
    }

    #[tokio::test]
    async fn consulta_curta_e_ignorada() {
        let port = 46000 + (rand::random::<u16>() % 1000);
        let _responder = respond_broadcast("aaa".into(), 41414, port).expect("responder");
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("socket");
        socket.send_to(b"MOUSEX?*", (Ipv4Addr::LOCALHOST, port)).await.expect("envio");
        let mut buffer = [0u8; 256];
        assert!(tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buffer)).await.is_err());
    }

    #[test]
    fn so_responde_rede_local() {
        assert!(is_local("192.168.1.50".parse().expect("ip")));
        assert!(is_local("10.1.2.3".parse().expect("ip")));
        assert!(is_local("127.0.0.1".parse().expect("ip")));
        assert!(!is_local("8.8.8.8".parse().expect("ip")));
    }

    #[cfg(windows)]
    fn thread_count() -> usize {
        let script = format!("(Get-Process -Id {}).Threads.Count", std::process::id());
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .expect("powershell");
        String::from_utf8_lossy(&output.stdout).trim().parse().expect("número de threads")
    }

    /// Busca cancelada (timeout/broadcast vence) não pode deixar o daemon mDNS vivo.
    #[cfg(windows)]
    #[tokio::test]
    async fn busca_mdns_cancelada_nao_vaza_threads() {
        let cache = temp_cache();
        let before = thread_count();
        for _ in 0..15 {
            let _ = find_with("fp-inexistente", &cache, Duration::from_millis(50), 47999, true).await;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let after = thread_count();
        assert!(after <= before + 3, "threads: antes {before}, depois {after}");
    }

    /// Usa a rede real (mDNS); rodar manualmente: cargo test -- --ignored mdns.
    #[tokio::test]
    #[ignore]
    async fn mdns_encontra_anunciante() {
        let mdns = mdns_sd::ServiceDaemon::new().expect("mdns");
        let info = mdns_sd::ServiceInfo::new(
            SERVICE,
            "teste-mousex",
            "teste-mousex.local.",
            "",
            41999,
            &[("fp", "fp-mdns")][..],
        )
        .expect("info")
        .enable_addr_auto();
        mdns.register(info).expect("registro");
        let found = tokio::time::timeout(Duration::from_secs(5), browse_mdns("fp-mdns"))
            .await
            .expect("timeout")
            .expect("encontrado");
        assert_eq!(found.port(), 41999);
        let _ = mdns.shutdown();
    }
}
