//! Atualização automática: decide se uma release é mais nova e se ela é autêntica.
//! A rede, a troca de arquivos e o reinício ficam no daemon; aqui só o que dá para testar sem efeitos.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// Chave pública das releases oficiais. A privada fica fora do repositório (secret da CI).
pub const RELEASE_PUBLIC_KEY: [u8; 32] = [
    0x2e, 0x45, 0x06, 0x9b, 0xe0, 0x61, 0x7c, 0x6b, 0xfd, 0x43, 0xca, 0x08, 0x87, 0x26, 0x3a, 0x6d, 0x59, 0x0d, 0x68,
    0x42, 0xc7, 0x07, 0x5f, 0x68, 0x71, 0x4b, 0xba, 0x84, 0xd9, 0xd1, 0xbe, 0xc9,
];

/// "v1.2.3" → (1, 2, 3). Tags com sufixo (pré-release) não são atualização automática.
pub fn parse_tag(tag: &str) -> Option<(u32, u32, u32)> {
    let mut parts = tag.strip_prefix('v')?.split('.').map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// A tag é mais nova que a versão em execução ("1.0.0", sem o v)?
pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_tag(tag), parse_tag(&format!("v{current}"))) {
        (Some(remote), Some(local)) => remote > local,
        _ => false,
    }
}

/// O que a CI assina: a tag junto com o SHA256SUMS, para uma release antiga não passar por nova.
pub fn signed_message(tag: &str, sums: &[u8]) -> Vec<u8> {
    [format!("MouseX {tag}\n").as_bytes(), sums].concat()
}

/// Confere a assinatura Ed25519 (64 bytes) do SHA256SUMS da `tag` com a `key` dada.
pub fn verify(key: &[u8; 32], tag: &str, sums: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_bytes(key), Signature::from_slice(signature)) else {
        return false;
    };
    key.verify(&signed_message(tag, sums), &signature).is_ok()
}

/// Hash esperado de `file` numa linha "<sha256>  <arquivo>" do SHA256SUMS.
pub fn expected_sha256(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        (name.trim_start().trim_start_matches('*') == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex")).collect()
    }

    // Vetor gerado com `openssl pkeyutl -sign -rawin`, o mesmo comando da CI, e uma chave descartável.
    const SUMS: &[u8] = b"aaaa  MouseX-windows-x64.zip\nbbbb  MouseX-macos-universal.tar.gz\n";
    const TEST_KEY: &str = "611cb38103d8c0267a7c302cf21b64d8af59600a105a9e79ee7d5e419815eaf0";
    const TEST_SIG: &str = "301d5fe418309e37b7e02c69821b93b659180e63662ae11ed772db06a08fe623\
                            b8e8744f705eeefa0c672005feef4bf0762a13bf2f96d3fb5b32aa2aad72e905";

    fn key() -> [u8; 32] {
        hex(TEST_KEY).try_into().expect("32 bytes")
    }

    #[test]
    fn assinatura_feita_pelo_openssl_da_ci_e_aceita() {
        assert!(verify(&key(), "v1.2.3", SUMS, &hex(TEST_SIG)));
    }

    #[test]
    fn release_antiga_rotulada_como_nova_e_recusada() {
        assert!(!verify(&key(), "v9.9.9", SUMS, &hex(TEST_SIG)), "a tag faz parte do que é assinado");
    }

    #[test]
    fn soma_alterada_ou_chave_errada_ou_lixo_sao_recusados() {
        assert!(!verify(&key(), "v1.2.3", b"cccc  MouseX-windows-x64.zip\n", &hex(TEST_SIG)));
        assert!(!verify(&RELEASE_PUBLIC_KEY, "v1.2.3", SUMS, &hex(TEST_SIG)));
        assert!(!verify(&key(), "v1.2.3", SUMS, b"curta"));
    }

    #[test]
    fn so_versao_maior_e_atualizacao() {
        assert!(is_newer("v1.1.0", "1.0.0"));
        assert!(is_newer("v1.0.10", "1.0.9"), "comparação numérica, não de texto");
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("v0.9.0", "1.0.0"));
        assert!(!is_newer("v2.0.0-beta.1", "1.0.0"), "pré-release não entra sozinha");
        assert!(!is_newer("latest", "1.0.0"));
        assert_eq!(parse_tag("v1.2"), None);
        assert_eq!(parse_tag("v1.2.3.4"), None);
    }

    #[test]
    fn hash_do_pacote_vem_da_linha_certa() {
        let sums = "AAAA0000000000000000000000000000000000000000000000000000000000ff  MouseX-windows-x64.zip\n\
                    bbbb000000000000000000000000000000000000000000000000000000000000 *install.sh\n";
        assert_eq!(
            expected_sha256(sums, "MouseX-windows-x64.zip").as_deref(),
            Some("aaaa0000000000000000000000000000000000000000000000000000000000ff")
        );
        assert!(expected_sha256(sums, "install.sh").is_some(), "aceita o * do modo binário");
        assert_eq!(expected_sha256(sums, "MouseX-windows-x64"), None);
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
