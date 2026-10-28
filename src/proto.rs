//! Protocolo do daemon: mensagens de controle (stream confiável) e movimento (datagrama).
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Versão deste build (workspace inteiro tem a mesma).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_CONTROL_BYTES: usize = 64 * 1024;
const MOTION_TAG: u8 = 0x4D;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Control {
    /// Cliente → servidor, ao conectar e quando os monitores mudam.
    Hello {
        machine: String,
        monitors: Vec<MonitorInfo>,
        /// Versão do MouseX do cliente, para o menu do servidor. Clientes antigos não enviam.
        #[serde(default)]
        version: Option<String>,
    },
    /// Servidor → cliente: cursor entrou; coordenadas locais do monitor.
    Enter {
        monitor_id: String,
        x: i32,
        y: i32,
    },
    /// Servidor → cliente: soltar tudo.
    Leave,
    Key {
        hid: u16,
        down: bool,
    },
    Button {
        button: u8,
        down: bool,
        monitor_id: String,
        x: i32,
        y: i32,
    },
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// Servidor → cliente: o cursor saiu dele; enviar o clipboard pelo stream de clipboard se mudou.
    ClipboardPull,
    /// Servidor → cliente: o usuário está ativo em alguma máquina; não bloquear a tela por inatividade.
    Activity,
    /// Servidor → cliente: Win+L no servidor; bloquear a tela agora.
    Lock,
    /// Servidor → cliente: mostrar o número de cada monitor (id nativo → número) por alguns segundos.
    /// Cliente antigo não conhece a variante e `read_known` a pula, sem desconectar.
    Identify {
        screens: Vec<(String, u32)>,
    },
    /// Servidor → cliente, após o pareamento: nome do servidor, para o ícone mostrar "Conectado a …".
    /// Versões antigas não enviam; quem não conhece a mensagem a pula (`read_known`).
    Server {
        machine: String,
    },
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Frame: u32 LE de comprimento + JSON.
// ponytail: JSON no canal de controle (eventos de baixa taxa); binário só no movimento, que é o caminho quente.
pub async fn write_control(writer: &mut (impl AsyncWrite + Unpin), message: &Control) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(|e| invalid(&e.to_string()))?;
    if body.len() > MAX_CONTROL_BYTES {
        return Err(invalid("control message above the size limit"));
    }
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    writer.write_all(&frame).await?;
    writer.flush().await
}

pub async fn read_control(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Control> {
    let length = reader.read_u32_le().await? as usize;
    if length == 0 || length > MAX_CONTROL_BYTES {
        return Err(invalid("invalid control length"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    // Frame inteiro já consumido: mensagem de versão mais nova vira `Unsupported` e o leitor segue no próximo.
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e.to_string()))
}

/// Como `read_control`, pulando mensagens que esta versão não conhece (par mais novo durante uma atualização).
pub async fn read_known(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Control> {
    loop {
        match read_control(reader).await {
            Err(error) if error.kind() == io::ErrorKind::Unsupported => continue,
            other => return other,
        }
    }
}

/// Posição absoluta no monitor `monitor` (índice em `Hello.monitors`); perdido = ignorado, sem deriva.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub seq: u32,
    pub monitor: u16,
    pub x: i32,
    pub y: i32,
}

impl Motion {
    pub fn encode(&self) -> [u8; 15] {
        let mut out = [0u8; 15];
        out[0] = MOTION_TAG;
        out[1..5].copy_from_slice(&self.seq.to_le_bytes());
        out[5..7].copy_from_slice(&self.monitor.to_le_bytes());
        out[7..11].copy_from_slice(&self.x.to_le_bytes());
        out[11..15].copy_from_slice(&self.y.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 15] = bytes.try_into().ok()?;
        if bytes[0] != MOTION_TAG {
            return None;
        }
        let word = |range: std::ops::Range<usize>| -> [u8; 4] { bytes[range].try_into().expect("fatia fixa") };
        Some(Self {
            seq: u32::from_le_bytes(word(1..5)),
            monitor: u16::from_le_bytes([bytes[5], bytes[6]]),
            x: i32::from_le_bytes(word(7..11)),
            y: i32::from_le_bytes(word(11..15)),
        })
    }
}

/// `seq` é mais novo que `last`, tolerando wrap-around.
pub fn is_newer(seq: u32, last: u32) -> bool {
    (seq.wrapping_sub(last) as i32) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Control> {
        vec![
            Control::Hello {
                machine: "win2".into(),
                monitors: vec![MonitorInfo { id: "m1".into(), x: -1920, y: 0, width: 1920, height: 1080, scale: 1.5 }],
                version: Some("1.4.0".into()),
            },
            Control::Enter { monitor_id: "m1".into(), x: 0, y: 333 },
            Control::Leave,
            Control::Key { hid: 0x04, down: true },
            Control::Button { button: 1, down: false, monitor_id: "m1".into(), x: 10, y: 20 },
            Control::Wheel { dx: 0, dy: -120 },
            Control::ClipboardPull,
            Control::Activity,
            Control::Lock,
            Control::Identify { screens: vec![("m1".into(), 1), ("m2".into(), 2)] },
        ]
    }

    #[tokio::test]
    async fn control_roundtrip_todas_variantes() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        for message in all() {
            write_control(&mut a, &message).await.expect("escrita");
        }
        for message in all() {
            assert_eq!(read_control(&mut b).await.expect("leitura"), message);
        }
    }

    #[tokio::test]
    async fn mensagem_de_versao_mais_nova_e_pulada_sem_perder_o_fluxo() {
        let unknown = br#"{"Novidade":{"x":1}}"#;
        let mut frames = (unknown.len() as u32).to_le_bytes().to_vec();
        frames.extend(unknown);
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&frames).await.expect("desconhecida");
        write_control(&mut a, &Control::Leave).await.expect("conhecida");
        assert_eq!(read_control(&mut b).await.expect_err("desconhecida").kind(), io::ErrorKind::Unsupported);
        assert_eq!(read_control(&mut b).await.expect("segue lendo"), Control::Leave);
    }

    #[tokio::test]
    async fn control_acima_de_64k_rejeitado() {
        let huge = Control::Hello { machine: "x".repeat(MAX_CONTROL_BYTES + 1), monitors: vec![], version: None };
        let (mut a, _b) = tokio::io::duplex(1024);
        assert!(write_control(&mut a, &huge).await.is_err());
        let header = ((MAX_CONTROL_BYTES + 1) as u32).to_le_bytes();
        let mut forged = header.as_slice();
        assert!(read_control(&mut forged).await.is_err());
    }

    #[test]
    fn hello_de_cliente_antigo_chega_sem_versao() {
        let old = r#"{"Hello":{"machine":"mac","monitors":[]}}"#;
        let hello: Control = serde_json::from_str(old).expect("hello antigo");
        assert_eq!(hello, Control::Hello { machine: "mac".into(), monitors: vec![], version: None });
    }

    #[test]
    fn motion_roundtrip() {
        let motion = Motion { seq: 7, monitor: 1, x: -5, y: 1080 };
        assert_eq!(Motion::decode(&motion.encode()), Some(motion));
    }

    #[test]
    fn motion_tamanho_errado_e_none() {
        let bytes = Motion { seq: 1, monitor: 0, x: 0, y: 0 }.encode();
        assert_eq!(Motion::decode(&bytes[..14]), None);
        assert_eq!(Motion::decode(&[bytes.as_slice(), &[0]].concat()), None);
        let mut wrong = bytes;
        wrong[0] = 0;
        assert_eq!(Motion::decode(&wrong), None);
    }

    #[test]
    fn seq_wraparound() {
        assert!(is_newer(0, u32::MAX));
        assert!(!is_newer(5, 6));
        assert!(!is_newer(6, 6));
        assert!(is_newer(7, 6));
    }
}
