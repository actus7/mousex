//! Pareamento humano; allowlist persistente é a única autorização durável.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

pub const CODE_MODULUS: u32 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("storage: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown peer: {0}")]
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRecord {
    pub fingerprint: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AllowlistFile {
    peers: Vec<PeerRecord>,
}

pub struct Allowlist {
    path: std::path::PathBuf,
    file: AllowlistFile,
}

impl Allowlist {
    pub fn load(directory: &Path) -> Result<Self, PairingError> {
        fs::create_dir_all(directory)?;
        let path = directory.join("allowlist.json");
        let file = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => AllowlistFile::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, file })
    }

    pub fn is_allowed(&self, fingerprint: &str) -> bool {
        self.file.peers.iter().any(|peer| peer.fingerprint == fingerprint)
    }

    pub fn list(&self) -> &[PeerRecord] {
        &self.file.peers
    }

    /// Confirmação humana é bool explícito; sem ela nada persiste.
    pub fn confirm(&mut self, record: PeerRecord, human_confirmed: bool) -> Result<bool, PairingError> {
        if !human_confirmed {
            return Ok(false);
        }
        if !self.is_allowed(&record.fingerprint) {
            self.file.peers.push(record);
            self.save()?;
        }
        Ok(true)
    }

    pub fn revoke(&mut self, fingerprint: &str) -> Result<bool, PairingError> {
        let before = self.file.peers.len();
        self.file.peers.retain(|peer| peer.fingerprint != fingerprint);
        let removed = self.file.peers.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<(), PairingError> {
        fs::write(&self.path, serde_json::to_vec_pretty(&self.file)?)?;
        Ok(())
    }
}

/// HMAC-SHA256 truncado a 6 dígitos do segredo da sessão TLS.
pub fn pairing_code(session_secret: &[u8], peer_fingerprint: &str) -> u32 {
    const BLOCK: usize = 64;
    let mut key = [0u8; BLOCK];
    if session_secret.len() > BLOCK {
        key[..32].copy_from_slice(&Sha256::digest(session_secret));
    } else {
        key[..session_secret.len()].copy_from_slice(session_secret);
    }
    let inner: Vec<u8> = key.iter().map(|b| b ^ 0x36).collect();
    let outer: Vec<u8> = key.iter().map(|b| b ^ 0x5c).collect();
    let mut inner_hash = Sha256::new();
    inner_hash.update(inner);
    inner_hash.update(peer_fingerprint.as_bytes());
    let inner_digest = inner_hash.finalize();
    let mut outer_hash = Sha256::new();
    outer_hash.update(outer);
    outer_hash.update(inner_digest);
    let digest = outer_hash.finalize();
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % CODE_MODULUS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_peer_rejected_then_confirmed_and_revoked() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("mousex-pair-{}", rand::random::<u64>()));
        let mut first = Allowlist::load(&dir)?;
        assert!(!first.is_allowed("ab"), "desconhecido deve ser rejeitado");
        assert!(
            !first.confirm(PeerRecord { fingerprint: "ab".into(), name: "laptop".into() }, false)?,
            "sem confirmação humana nada grava"
        );
        assert!(!first.is_allowed("ab"));
        assert!(first.confirm(PeerRecord { fingerprint: "ab".into(), name: "laptop".into() }, true)?);
        // Reconexão direta: allowlist recarregada do disco autoriza sem novo pareamento.
        let second = Allowlist::load(&dir)?;
        assert!(second.is_allowed("ab"));
        assert_eq!(second.list(), [PeerRecord { fingerprint: "ab".into(), name: "laptop".into() }]);
        let mut third = Allowlist::load(&dir)?;
        assert!(third.revoke("ab")?);
        assert!(!third.revoke("ab")?);
        assert!(!Allowlist::load(&dir)?.is_allowed("ab"));
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn code_is_six_digits_and_secret_bound() {
        let code = pairing_code(b"session-secret", "ab");
        assert!(code < CODE_MODULUS);
        assert_ne!(code, pairing_code(b"other-secret", "ab"));
        assert_ne!(code, pairing_code(b"session-secret", "cd"));
    }
}
