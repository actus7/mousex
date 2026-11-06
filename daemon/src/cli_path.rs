//! Mac e Linux: `mousex` funciona em qualquer terminal novo.
//! O agente confere isso toda vez que sobe, e todo update reinicia o agente. No Windows quem faz isso é
//! `install::refresh_update_task` (PATH do sistema).
use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

const LOCAL_BIN: &str = ".local/bin";
const CLI: &str = "mousex";
const MARKER: &str = "# MouseX: pasta do comando mousex";
const PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";
/// Onde o usuário costuma pôr o PATH; qualquer menção a `.local/bin` nesses arquivos já basta.
const PROFILES: [&str; 6] = [".zshrc", ".zprofile", ".zshenv", ".bashrc", ".bash_profile", ".profile"];
/// Arquivos que ganham a linha quando nenhum perfil menciona a pasta.
const APPEND_TO: [&str; 2] = [".zshrc", ".bashrc"];
#[cfg(target_os = "macos")]
const DEFAULT_PROFILE: &str = ".zshrc";
#[cfg(not(target_os = "macos"))]
const DEFAULT_PROFILE: &str = ".bashrc";

/// Chamado na subida do agente: erro vira log, nunca derruba o agente.
pub fn ensure_logged() {
    let Some(home) = dirs::home_dir() else { return };
    let cli = crate::config::data_dir().join("bin").join(CLI);
    if !cli.exists() {
        return;
    }
    if let Err(error) = ensure(&home, &cli) {
        crate::log(&format!("mousex command not added to PATH: {error}"));
    }
}

/// Link `~/.local/bin/mousex` para o binário instalado, e a pasta no PATH dos terminais se faltar.
pub fn ensure(home: &Path, cli: &Path) -> io::Result<()> {
    ensure_link(&home.join(LOCAL_BIN), cli)?;
    ensure_path_line(home)
}

fn ensure_link(dir: &Path, cli: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let link = dir.join(CLI);
    match fs::symlink_metadata(&link) {
        // Já resolve para algum `mousex` (o nosso ou um que o usuário escolheu): não mexe.
        Ok(_) if link.exists() => return Ok(()),
        // Link quebrado (instalação antiga ou apagada): troca.
        Ok(_) => fs::remove_file(&link)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    symlink(cli, link)
}

fn ensure_path_line(home: &Path) -> io::Result<()> {
    let profile = |name: &str| fs::read_to_string(home.join(name)).ok();
    if PROFILES.iter().filter_map(|name| profile(name)).any(|text| text.contains(LOCAL_BIN)) {
        return Ok(());
    }
    let mut targets: Vec<PathBuf> = APPEND_TO.iter().map(|name| home.join(name)).filter(|path| path.exists()).collect();
    if targets.is_empty() {
        targets.push(home.join(DEFAULT_PROFILE));
    }
    for path in targets {
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("\n{MARKER}\n{PATH_LINE}\n"));
        fs::write(path, text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_home() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mousex-cli-path-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_cli(home: &Path) -> PathBuf {
        let cli = home.join("app-bin-mousex");
        fs::write(&cli, "x").unwrap();
        cli
    }

    #[test]
    fn cria_o_link_e_a_linha_de_path_quando_faltam() {
        let home = temp_home();
        let cli = fake_cli(&home);
        ensure(&home, &cli).unwrap();
        assert_eq!(fs::read_link(home.join(".local/bin/mousex")).unwrap(), cli);
        let profile = fs::read_to_string(home.join(DEFAULT_PROFILE)).unwrap();
        assert!(profile.contains(PATH_LINE), "{profile}");
    }

    #[test]
    fn nao_mexe_no_perfil_quando_o_usuario_ja_tem_a_pasta_no_path() {
        let home = temp_home();
        let cli = fake_cli(&home);
        fs::write(home.join(".zprofile"), "export PATH=$HOME/.local/bin:$PATH\n").unwrap();
        ensure(&home, &cli).unwrap();
        assert!(home.join(".local/bin/mousex").exists());
        assert!(!home.join(".zshrc").exists() && !home.join(".bashrc").exists());
    }

    #[test]
    fn rodar_de_novo_nao_duplica_nada() {
        let home = temp_home();
        let cli = fake_cli(&home);
        ensure(&home, &cli).unwrap();
        ensure(&home, &cli).unwrap();
        let profile = fs::read_to_string(home.join(DEFAULT_PROFILE)).unwrap();
        assert_eq!(profile.matches(MARKER).count(), 1, "{profile}");
    }

    #[test]
    fn troca_link_quebrado_e_preserva_arquivo_do_usuario() {
        let home = temp_home();
        let cli = fake_cli(&home);
        let dir = home.join(".local/bin");
        fs::create_dir_all(&dir).unwrap();
        symlink(home.join("sumiu"), dir.join(CLI)).unwrap();
        ensure_link(&dir, &cli).unwrap();
        assert_eq!(fs::read_link(dir.join(CLI)).unwrap(), cli, "link quebrado é trocado");

        let other = temp_home();
        fs::create_dir_all(other.join("bin")).unwrap();
        fs::write(other.join("bin").join(CLI), "meu").unwrap();
        ensure_link(&other.join("bin"), &cli).unwrap();
        assert_eq!(fs::read_to_string(other.join("bin").join(CLI)).unwrap(), "meu", "arquivo do usuário fica");
    }
}
