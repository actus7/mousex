//! QUIC (quinn) com certificado pinado por fingerprint: datagramas para movimento, streams para o resto.
use crate::identity::DeviceIdentity;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};

pub const PORT: u16 = 41414;
pub const ALPN: &[u8] = b"mousex/1";
const KEEPALIVE: Duration = Duration::from_millis(500);
const IDLE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Primeiro byte do stream de controle: torna o stream visível ao servidor.
const CONTROL_PREAMBLE: u8 = 0x53;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("network: {0}")]
    Io(#[from] io::Error),
    #[error("connection: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("connection: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("stream: {0}")]
    Write(#[from] quinn::WriteError),
    #[error("stream: {0}")]
    Read(#[from] quinn::ReadExactError),
    #[error("endpoint closed")]
    Closed,
    #[error("peer sent no certificate")]
    NoCertificate,
    #[error("invalid control preamble")]
    Preamble,
}

pub struct Link {
    pub conn: quinn::Connection,
    pub peer_fingerprint: String,
    pub control_tx: quinn::SendStream,
    pub control_rx: quinn::RecvStream,
}

// Qualquer certificado é aceito no handshake (assinaturas verificadas); autorização é pela allowlist depois.
#[derive(Debug)]
struct PinLater(Arc<rustls::crypto::CryptoProvider>);

impl PinLater {
    fn verify(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, signature, &self.0.signature_verification_algorithms)
    }
    fn schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl rustls::client::danger::ServerCertVerifier for PinLater {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 não suportado".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.verify(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl rustls::server::danger::ClientCertVerifier for PinLater {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 não suportado".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.verify(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn credentials(identity: &DeviceIdentity) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    (
        vec![CertificateDer::from(identity.certificate.clone())],
        PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec()).into(),
    )
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(KEEPALIVE));
    config.max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("timeout válido")));
    Arc::new(config)
}

pub fn server_endpoint(identity: &DeviceIdentity, port: u16) -> io::Result<quinn::Endpoint> {
    let (cert, key) = credentials(identity);
    let provider = provider();
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(other)?
        .with_client_cert_verifier(Arc::new(PinLater(provider)))
        .with_single_cert(cert, key)
        .map_err(other)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(other)?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport());
    quinn::Endpoint::server(config, SocketAddr::from(([0, 0, 0, 0], port)))
}

pub fn client_endpoint(identity: &DeviceIdentity) -> io::Result<quinn::Endpoint> {
    let (cert, key) = credentials(identity);
    let provider = provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(other)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinLater(provider)))
        .with_client_auth_cert(cert, key)
        .map_err(other)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(other)?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport());
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

fn fingerprint_of(conn: &quinn::Connection) -> Result<String, NetError> {
    let certs = conn
        .peer_identity()
        .and_then(|any| any.downcast::<Vec<CertificateDer<'static>>>().ok())
        .ok_or(NetError::NoCertificate)?;
    let leaf = certs.first().ok_or(NetError::NoCertificate)?;
    Ok(Sha256::digest(leaf.as_ref()).iter().map(|byte| format!("{byte:02x}")).collect())
}

/// O cliente abre o stream de controle; o servidor espera o preâmbulo.
pub async fn accept(endpoint: &quinn::Endpoint) -> Result<Link, NetError> {
    let conn = endpoint.accept().await.ok_or(NetError::Closed)?.await?;
    let peer_fingerprint = fingerprint_of(&conn)?;
    let (control_tx, mut control_rx) = conn.accept_bi().await?;
    let mut preamble = [0u8; 1];
    control_rx.read_exact(&mut preamble).await?;
    if preamble[0] != CONTROL_PREAMBLE {
        return Err(NetError::Preamble);
    }
    Ok(Link { conn, peer_fingerprint, control_tx, control_rx })
}

pub async fn connect(endpoint: &quinn::Endpoint, address: SocketAddr) -> Result<Link, NetError> {
    let conn = endpoint.connect(address, "mousex.local")?.await?;
    let peer_fingerprint = fingerprint_of(&conn)?;
    let (mut control_tx, control_rx) = conn.open_bi().await?;
    control_tx.write_all(&[CONTROL_PREAMBLE]).await?;
    Ok(Link { conn, peer_fingerprint, control_tx, control_rx })
}

/// Segredo da sessão TLS; igual nos dois lados, base do código de pareamento.
pub fn pairing_secret(conn: &quinn::Connection) -> [u8; 32] {
    let mut secret = [0u8; 32];
    conn.export_keying_material(&mut secret, b"mousex pairing v1", b"").expect("exportador TLS 1.3");
    secret
}

pub async fn open_clipboard_stream(
    conn: &quinn::Connection,
) -> Result<(quinn::SendStream, quinn::RecvStream), NetError> {
    Ok(conn.open_bi().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Control, Motion, read_control, write_control};
    use std::time::Instant;

    fn identities() -> (DeviceIdentity, DeviceIdentity, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("mousex-net-{}", rand::random::<u64>()));
        let host = DeviceIdentity::load_or_create(&root.join("host")).expect("host");
        let client = DeviceIdentity::load_or_create(&root.join("client")).expect("client");
        (host, client, root)
    }

    async fn pair() -> (Link, Link, quinn::Endpoint, quinn::Endpoint, DeviceIdentity, DeviceIdentity) {
        let (host, client, root) = identities();
        let server = server_endpoint(&host, 0).expect("server");
        let port = server.local_addr().expect("addr").port();
        let client_ep = client_endpoint(&client).expect("client ep");
        let address: SocketAddr = ([127, 0, 0, 1], port).into();
        let (a, b) = tokio::join!(accept(&server), connect(&client_ep, address));
        std::fs::remove_dir_all(root).ok();
        (a.expect("accept"), b.expect("connect"), server, client_ep, host, client)
    }

    #[tokio::test]
    async fn handshake_expoe_fingerprints() {
        let (server_link, client_link, _s, _c, host, client) = pair().await;
        assert_eq!(server_link.peer_fingerprint, client.fingerprint);
        assert_eq!(client_link.peer_fingerprint, host.fingerprint);
    }

    #[tokio::test]
    async fn datagrama_chega() {
        let (server_link, client_link, _s, _c, _, _) = pair().await;
        let motion = Motion { seq: 1, monitor: 0, x: 10, y: 20 };
        server_link.conn.send_datagram(motion.encode().to_vec().into()).expect("datagrama");
        let received = client_link.conn.read_datagram().await.expect("recebido");
        assert_eq!(Motion::decode(&received), Some(motion));
    }

    #[tokio::test]
    async fn controle_ordenado() {
        let (mut server_link, mut client_link, _s, _c, _, _) = pair().await;
        for i in 0..100u16 {
            write_control(&mut server_link.control_tx, &Control::Key { hid: i, down: true }).await.expect("envio");
        }
        for i in 0..100u16 {
            assert_eq!(
                read_control(&mut client_link.control_rx).await.expect("leitura"),
                Control::Key { hid: i, down: true }
            );
        }
    }

    #[tokio::test]
    async fn codigo_de_pareamento_igual_nos_dois_lados() {
        let (server_link, client_link, _s, _c, _, client) = pair().await;
        let a = crate::pairing::pairing_code(&pairing_secret(&server_link.conn), &client.fingerprint);
        let b = crate::pairing::pairing_code(&pairing_secret(&client_link.conn), &client.fingerprint);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn queda_detectada_em_menos_de_2s() {
        let (server_link, client_link, _s, client_ep, _, _) = pair().await;
        let start = Instant::now();
        drop(client_link);
        drop(client_ep);
        tokio::time::timeout(Duration::from_secs(2), server_link.conn.closed())
            .await
            .expect("queda não detectada em 2 s");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
