//! Arquivos no clipboard do Windows (CF_HDROP): estrutura DROPFILES seguida dos caminhos em UTF-16.
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

/// Tamanho de DROPFILES: pFiles (u32), pt (2 × i32), fNC (u32), fWide (u32).
const HEADER: usize = 20;

/// DROPFILES em UTF-16: cada caminho terminado em NUL e um NUL a mais no fim da lista.
pub fn build_dropfiles(paths: &[PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + paths.len() * 64);
    bytes.extend((HEADER as u32).to_le_bytes());
    bytes.extend([0u8; 12]);
    bytes.extend(1u32.to_le_bytes());
    for unit in paths.iter().flat_map(|p| p.as_os_str().encode_wide().chain(Some(0))).chain(Some(0)) {
        bytes.extend(unit.to_le_bytes());
    }
    bytes
}

/// Caminhos de um bloco CF_HDROP; `None` se malformado, ANSI ou vazio.
pub fn parse_dropfiles(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    let offset = word(0)? as usize;
    if word(16)? == 0 || offset < HEADER || offset > bytes.len() {
        return None;
    }
    let units: Vec<u16> = bytes[offset..].as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    let paths: Vec<PathBuf> = units
        .split(|&unit| unit == 0)
        .take_while(|name| !name.is_empty())
        .map(|name| PathBuf::from(OsString::from_wide(name)))
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// Caminhos comuns do Explorer: unidade ("C:\…") ou rede ("\\servidor\…"). Nada relativo nem de dispositivo
/// ("\\?\GLOBALROOT\…", "\\.\…"), que levaria o agente a ler cópias de sombra, pipes ou volumes crus.
pub fn usable(paths: &[PathBuf]) -> bool {
    use std::path::{Component, Prefix};
    let plain = |path: &Path| {
        matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::UNC(..)))
            && path.is_absolute()
    };
    !paths.is_empty() && paths.iter().all(|p| plain(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn dropfiles_ida_e_volta() {
        let paths = vec![PathBuf::from(r"C:\Users\a\Área de Trabalho\nota.txt"), PathBuf::from(r"C:\dados\pasta")];
        let bytes = build_dropfiles(&paths);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 20, "caminhos logo após o cabeçalho");
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1, "UTF-16");
        assert_eq!(parse_dropfiles(&bytes), Some(paths));
    }

    #[test]
    fn so_caminhos_comuns_de_disco_ou_rede() {
        assert!(usable(&[PathBuf::from(r"C:\a\b.txt"), PathBuf::from(r"\\servidor\pasta\c.txt")]));
        for bad in [
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\x",
            r"\\?\C:\a",
            r"\\.\pipe\x",
            r"relativo\a",
            r"\sem-unidade",
        ] {
            assert!(!usable(&[PathBuf::from(bad)]), "{bad}");
        }
        assert!(!usable(&[]));
    }

    #[test]
    fn dropfiles_invalido_e_recusado() {
        assert_eq!(parse_dropfiles(&[0; 8]), None, "menor que o cabeçalho");
        let mut bytes = build_dropfiles(&[PathBuf::from(r"C:\a")]);
        bytes[0] = 200;
        assert_eq!(parse_dropfiles(&bytes), None, "deslocamento além do bloco");
        let ansi = [20u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'C', b':', 0, 0];
        assert_eq!(parse_dropfiles(&ansi), None, "só UTF-16 é aceito");
        assert_eq!(parse_dropfiles(&build_dropfiles(&[])), None, "lista vazia");
    }
}
