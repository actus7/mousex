//! Identidade persistente; certificados e chaves nunca são enviados em claro.
use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("storage: {0}")]
    Io(#[from] io::Error),
    #[error("invalid identity: {0}")]
    Invalid(String),
}

pub struct DeviceIdentity {
    pub certificate: Vec<u8>,
    private_key: Vec<u8>,
    pub fingerprint: String,
}

impl DeviceIdentity {
    pub fn private_key_der(&self) -> &[u8] {
        &self.private_key
    }

    pub fn load_or_create(directory: &Path) -> Result<Self, IdentityError> {
        fs::create_dir_all(directory)?;
        restrict(directory, true)?;
        let key_path = directory.join("identity.pk8");
        let cert_path = directory.join("identity.der");
        let key_exists = key_path.exists();
        let cert_exists = cert_path.exists();
        if key_exists != cert_exists {
            return Err(IdentityError::Invalid("par de identidade incompleto; use reset explícito".into()));
        }
        if !key_exists {
            generate(&key_path, &cert_path)?;
        } else {
            // Confiança não é trocada silenciosamente: arquivo inválido exige reset humano.
            let key = fs::read(&key_path)?;
            let cert = fs::read(&cert_path)?;
            valid_pair(&key, &cert)
                .ok_or_else(|| IdentityError::Invalid("identidade persistida inválida; use reset explícito".into()))?;
        }
        restrict(&key_path, false)?;
        let private_key = fs::read(key_path)?;
        let certificate = fs::read(cert_path)?;
        Ok(Self { fingerprint: fingerprint(&certificate), certificate, private_key })
    }

    /// Único caminho de regeneração: remove material inválido por decisão explícita.
    pub fn reset_and_create(directory: &Path) -> Result<Self, IdentityError> {
        for name in ["identity.pk8", "identity.der"] {
            match fs::remove_file(directory.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Self::load_or_create(directory)
    }
}

fn generate(key_path: &Path, cert_path: &Path) -> Result<(), IdentityError> {
    let signing = SigningKey::generate(&mut OsRng);
    let private = signing.to_pkcs8_der().map_err(|e| IdentityError::Invalid(e.to_string()))?;
    let key = rcgen::KeyPair::try_from(private.as_bytes()).map_err(|e| IdentityError::Invalid(e.to_string()))?;
    let certificate = rcgen::CertificateParams::new(vec!["mousex.local".into()])
        .and_then(|params| params.self_signed(&key))
        .map_err(|e| IdentityError::Invalid(e.to_string()))?;
    // Diretório já restrito antes de criar qualquer material secreto.
    write_new(key_path, private.as_bytes())?;
    Ok(write_new(cert_path, certificate.der().as_ref())?)
}

/// Parse completo, correspondência chave↔cert e assinatura self-signed.
fn valid_pair(key: &[u8], cert: &[u8]) -> Option<()> {
    let pair = rcgen::KeyPair::try_from(key).ok()?;
    if pair.algorithm() != &rcgen::PKCS_ED25519 {
        return None;
    }
    let signing = SigningKey::from_pkcs8_der(key).ok()?;
    let raw_public = signing.verifying_key().to_bytes();
    let (_, parsed) = x509_parser::parse_x509_certificate(cert).ok()?;
    if parsed.public_key().subject_public_key.data.as_ref() != raw_public {
        return None;
    }
    let signature = ed25519_dalek::Signature::from_slice(parsed.signature_value.data.as_ref()).ok()?;
    use ed25519_dalek::Verifier;
    signing.verifying_key().verify(parsed.tbs_certificate.as_ref(), &signature).ok()
}

fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use io::Write;
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn restrict(path: &Path, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }))
}

#[cfg(windows)]
fn restrict(path: &Path, directory: bool) -> io::Result<()> {
    use std::process::Command;
    // Processo elevado (agente instalado): a pasta não pode ser gravável por programas comuns do mesmo usuário,
    // senão um junction plantado vira escrita elevada. SIDs fixos: nomes de grupo mudam com o idioma.
    if elevated() {
        let grants: &[&str] = if directory {
            &["*S-1-5-32-544:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX"]
        } else {
            &["*S-1-5-32-544:F", "*S-1-5-18:F"]
        };
        let mut command = Command::new("icacls");
        command.arg(path).args(["/inheritance:r", "/grant:r"]).args(grants);
        let output = command.output()?;
        if !output.status.success() {
            return Err(io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()));
        }
        return Ok(());
    }
    let output = Command::new("whoami").arg("/user").arg("/fo").arg("csv").arg("/nh").output()?;
    if !output.status.success() {
        return Err(io::Error::other("whoami failed"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let sid = text.trim().split(',').nth(1).ok_or_else(|| io::Error::other("SID unavailable"))?.trim_matches('"');
    let grant = format!("*{sid}:{}F", if directory { "(OI)(CI)" } else { "" });
    let output = Command::new("icacls").arg(path).args(["/inheritance:r", "/grant:r", &grant]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()));
    }
    Ok(())
}

/// Elevado = token com o grupo de nível de integridade alto (S-1-16-12288).
#[cfg(windows)]
fn elevated() -> bool {
    std::process::Command::new("whoami")
        .args(["/groups", "/fo", "csv", "/nh"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("S-1-16-12288"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupted_certificate_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("mousex-corrupt-{}", rand::random::<u64>()));
        DeviceIdentity::load_or_create(&path)?;
        fs::write(path.join("identity.der"), b"not a certificate")?;
        let error = match DeviceIdentity::load_or_create(&path) {
            Ok(_) => panic!("certificado corrompido deve falhar"),
            Err(e) => e,
        };
        assert!(matches!(error, IdentityError::Invalid(_)));
        assert!(path.join("identity.der").exists(), "reset silencioso é proibido");
        assert!(path.join("identity.pk8").exists(), "reset silencioso é proibido");
        let reset = DeviceIdentity::reset_and_create(&path)?;
        assert_eq!(reset.fingerprint.len(), 64);
        assert_ne!(reset.fingerprint, fingerprint(b"not a certificate"));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn mismatched_key_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("mousex-mismatch-{}", rand::random::<u64>()));
        DeviceIdentity::load_or_create(&path)?;
        let other = SigningKey::generate(&mut OsRng);
        let replacement = other.to_pkcs8_der().map_err(|e| IdentityError::Invalid(e.to_string()))?;
        fs::write(path.join("identity.pk8"), replacement.as_bytes())?;
        assert!(matches!(DeviceIdentity::load_or_create(&path), Err(IdentityError::Invalid(_))));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn incomplete_pair_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("mousex-incomplete-{}", rand::random::<u64>()));
        DeviceIdentity::load_or_create(&path)?;
        fs::remove_file(path.join("identity.der"))?;
        assert!(matches!(DeviceIdentity::load_or_create(&path), Err(IdentityError::Invalid(_))));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn generation_and_persistence() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("mousex-identity-{}", rand::random::<u64>()));
        let first = DeviceIdentity::load_or_create(&path)?;
        let second = DeviceIdentity::load_or_create(&path)?;
        assert_eq!(first.fingerprint.len(), 64);
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.private_key_der(), second.private_key_der());
        assert_eq!(first.certificate, second.certificate);
        fs::remove_dir_all(path)?;
        Ok(())
    }
}
