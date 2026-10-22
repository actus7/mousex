//! Configuração persistida em `config.json` na pasta de dados (`data_dir`).
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub role: Role,
    pub machine: String,
    /// Cliente: servidor pareado. `None` até o primeiro pareamento.
    pub server_fingerprint: Option<String>,
    /// Troca Ctrl→Cmd etc. ao controlar um Mac (usado pelo cliente Mac).
    pub remap_modifiers: bool,
}

/// Pasta de dados: identidade, allowlist, layout, cache e log.
/// Windows instalado: `ProgramData\MouseX` (só administradores escrevem; o agente roda elevado).
/// Antes da instalação, e no Mac: pasta de dados do usuário.
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = system_data_dir().filter(|dir| dir.join("config.json").exists()) {
        return dir;
    }
    user_data_dir()
}

pub fn user_data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("mousex")
}

/// Pasta do usuário de quando o programa se chamava Sculpin (só para migrar).
pub fn legacy_user_data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("sculpin")
}

#[cfg(windows)]
pub fn system_data_dir() -> Option<PathBuf> {
    mousex_native::program_data_dir().ok().map(|dir| dir.join("MouseX"))
}

/// `ProgramData\Sculpin` de instalações antigas: protegida como a nova, então confiável para migrar.
#[cfg(windows)]
pub fn legacy_system_data_dir() -> Option<PathBuf> {
    mousex_native::program_data_dir().ok().map(|dir| dir.join("Sculpin"))
}

impl Config {
    pub fn load(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(dir.join("config.json")).ok()?).ok()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let body = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temp = dir.join("config.json.tmp");
        std::fs::write(&temp, body)?;
        std::fs::rename(temp, dir.join("config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_e_ausente() {
        let dir = std::env::temp_dir().join(format!("mousex-config-{}", rand::random::<u64>()));
        assert!(Config::load(&dir).is_none());
        let config = Config {
            role: Role::Client,
            machine: "win2".into(),
            server_fingerprint: Some("ab".into()),
            remap_modifiers: true,
        };
        config.save(&dir).expect("salvar");
        let loaded = Config::load(&dir).expect("carregar");
        assert_eq!(
            (loaded.role, loaded.machine.as_str(), loaded.server_fingerprint.as_deref(), loaded.remap_modifiers),
            (Role::Client, "win2", Some("ab"), true)
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
