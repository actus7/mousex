//! `mousex update`: baixa a release mais nova, confere assinatura e hash, troca os binários e volta
//! atrás se o novo não abrir. Windows: tarefa como SYSTEM a cada 30 min. Mac e Linux: o agente chama a cada 30 min.
//! `curl` e `tar` são os do sistema (Windows 10+, macOS e as distribuições comuns já trazem os dois).
use crate::{
    config::{Config, data_dir},
    log,
};
use mousex_core::update::{RELEASE_PUBLIC_KEY, expected_sha256, is_newer, sha256_hex, verify};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

const RELEASES: &str = "https://github.com/actus7/mousex/releases";
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.sig";

#[cfg(windows)]
const PACKAGE: &str = "MouseX-windows-x64.zip";
#[cfg(windows)]
const FILES: &[&str] = &["mousex.exe", "mousex-agent.exe", "mousex-editor.exe", "WebView2Loader.dll"];
#[cfg(windows)]
const CLI: &str = "mousex.exe";
#[cfg(windows)]
const NULL: &str = "NUL";

#[cfg(target_os = "macos")]
const PACKAGE: &str = "MouseX-macos-universal.tar.gz";
#[cfg(target_os = "linux")]
const PACKAGE: &str = "MouseX-linux-x64.tar.gz";
#[cfg(not(windows))]
const FILES: &[&str] = &["mousex", "mousex-agent"];
#[cfg(not(windows))]
const CLI: &str = "mousex";
#[cfg(not(windows))]
const NULL: &str = "/dev/null";

pub fn update() -> Result<(), Box<dyn Error>> {
    if Config::load(&data_dir()).is_some_and(|c| !c.auto_update) {
        return Ok(());
    }
    let tag = latest_tag()?;
    let current = env!("CARGO_PKG_VERSION");
    if !is_newer(&tag, current) {
        return Ok(());
    }
    log(&format!("update: {current} -> {tag}"));
    let bin = bin_dir()?;
    // Área de trabalho dentro da pasta dos binários: só quem pode trocá-los escreve nela.
    let work = bin.join(".update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("package"))?;
    let verified = download_and_verify(&tag, &work);
    let stopped = verified.is_ok();
    let result = verified.and_then(|()| replace(&bin, &work.join("package"), &tag));
    let _ = std::fs::remove_dir_all(&work);
    match &result {
        Ok(()) => log(&format!("update: now running {tag}")),
        Err(error) => log(&format!("update to {tag} failed: {error}")),
    }
    // Por último: no Linux, reiniciar o serviço encerra também este processo (filho do agente).
    if stopped {
        start_agent();
    }
    result
}

/// `/releases/latest` redireciona para `/releases/tag/vX.Y.Z`: a tag sai da URL final, sem API nem limite.
fn latest_tag() -> Result<String, Box<dyn Error>> {
    let url = curl(&["-I", "-o", NULL, "-w", "%{url_effective}", &format!("{RELEASES}/latest")])?;
    Ok(url.rsplit_once("/tag/").map(|(_, tag)| tag.trim().to_string()).ok_or("latest release not found")?)
}

fn download_and_verify(tag: &str, work: &Path) -> Result<(), Box<dyn Error>> {
    for name in [PACKAGE, SUMS, SIGNATURE] {
        let target = work.join(name).display().to_string();
        curl(&["-o", &target, &format!("{RELEASES}/download/{tag}/{name}")])?;
    }
    let sums = std::fs::read(work.join(SUMS))?;
    if !verify(&RELEASE_PUBLIC_KEY, tag, &sums, &std::fs::read(work.join(SIGNATURE))?) {
        return Err("signature does not match the MouseX release key".into());
    }
    let expected =
        expected_sha256(&String::from_utf8_lossy(&sums), PACKAGE).ok_or("package missing from SHA256SUMS")?;
    if sha256_hex(&std::fs::read(work.join(PACKAGE))?) != expected {
        return Err("package hash does not match SHA256SUMS".into());
    }
    let package = work.join(PACKAGE).display().to_string();
    run("tar", &["-xf", &package, "-C", &work.join("package").display().to_string()])?;
    Ok(())
}

/// Troca cada binário guardando o anterior em `.old`; o novo precisa abrir e dizer a versão certa.
fn replace(bin: &Path, package: &Path, tag: &str) -> Result<(), Box<dyn Error>> {
    stop_agent()?;
    let names: Vec<&str> = FILES.iter().copied().filter(|name| package.join(name).exists()).collect();
    let (moved, swapped) = swap(bin, package, &names);
    let healthy = swapped.map_err(Box::<dyn Error>::from).and_then(|()| {
        sign(bin, &names);
        let version = version_of(&bin.join(CLI))?;
        if version == tag.trim_start_matches('v') {
            Ok(())
        } else {
            Err(format!("new binary reports version {version:?}").into())
        }
    });
    match healthy {
        Ok(()) => record_version(tag),
        Err(_) => restore(bin, &moved),
    }
    healthy
}

fn old(bin: &Path, name: &str) -> PathBuf {
    bin.join(format!("{name}.old"))
}

/// Copia o pacote por cima guardando cada anterior em `.old`. Devolve os que foram de fato renomeados
/// nesta vez: só eles voltam no rollback (um `.old` de update antigo nunca é restaurado).
fn swap<'a>(bin: &Path, package: &Path, names: &[&'a str]) -> (Vec<&'a str>, std::io::Result<()>) {
    let mut moved = Vec::new();
    let result = names.iter().try_for_each(|name| {
        let _ = std::fs::remove_file(old(bin, name));
        if bin.join(name).exists() {
            // Renomear funciona com o executável em uso (Windows) e preserva a assinatura em cache (Mac).
            std::fs::rename(bin.join(name), old(bin, name))?;
            moved.push(*name);
        }
        std::fs::copy(package.join(name), bin.join(name)).map(drop)
    });
    (moved, result)
}

fn restore(bin: &Path, moved: &[&str]) {
    for name in moved {
        let _ = std::fs::remove_file(bin.join(name));
        let _ = std::fs::rename(old(bin, name), bin.join(name));
    }
}

/// `mousex version` do binário novo, com prazo: um binário travado não pode prender o update (e o rollback).
fn version_of(cli: &Path) -> Result<String, Box<dyn Error>> {
    let mut child = Command::new(cli).arg("version").stdout(std::process::Stdio::piped()).spawn()?;
    for _ in 0..300 {
        if let Some(status) = child.try_wait()? {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut child.stdout.take().ok_or("no output")?, &mut text)?;
            return if status.success() { Ok(text.trim().to_string()) } else { Err("new binary failed to run".into()) };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    Err("new binary did not answer within 30 s".into())
}

fn curl(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let base = ["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "--max-time", "600"];
    run("curl", &[&base[..], args].concat())
}

fn run(program: &str, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(windows)]
fn bin_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(mousex_native::program_files_dir()?.join("MouseX"))
}

#[cfg(windows)]
fn stop_agent() -> Result<(), Box<dyn Error>> {
    let _ = run("schtasks", &["/End", "/TN", crate::install::TASK]);
    crate::logon_windows::stop_service()?;
    Ok(())
}

/// Sem ninguém logado a tarefa não sobe agora; ela sobe sozinha no próximo logon.
#[cfg(windows)]
fn start_agent() {
    if Config::load(&data_dir()).is_some_and(|c| c.role == crate::config::Role::Client)
        && let Ok(bin) = bin_dir()
        && let Err(error) = crate::logon_windows::install_service(&bin.join("mousex-agent.exe"))
    {
        log(&format!("update: logon service could not restart: {error}"));
    }
    let _ = run("schtasks", &["/Run", "/TN", crate::install::TASK]);
}

#[cfg(windows)]
fn sign(_: &Path, _: &[&str]) {}

/// Configurações → Aplicativos mostra a versão instalada; quem grava aqui ainda é o binário antigo.
#[cfg(windows)]
fn record_version(tag: &str) {
    let version = tag.trim_start_matches('v');
    let _ = run(
        "reg",
        &["add", crate::install::UNINSTALL_KEY, "/v", "DisplayVersion", "/t", "REG_SZ", "/d", version, "/f"],
    );
}

#[cfg(not(windows))]
fn record_version(_: &str) {}

#[cfg(not(windows))]
const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Mac e Linux: o agente roda `mousex update` 10 min depois de subir e depois a cada 30 min.
// ponytail: o sleep pausa com o Mac dormindo, então o intervalo pode esticar; com 30 min isso pouco importa.
#[cfg(not(windows))]
pub fn schedule() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(600));
        loop {
            if let Ok(dir) = bin_dir() {
                let _ = Command::new(dir.join(CLI)).arg("update").status();
            }
            std::thread::sleep(UPDATE_INTERVAL);
        }
    });
}

#[cfg(not(windows))]
fn bin_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(data_dir().join("bin"))
}

/// No Mac e no Linux o agente é quem chamou o update: ele só reinicia no fim.
#[cfg(not(windows))]
fn stop_agent() -> Result<(), Box<dyn Error>> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn start_agent() {
    if let Ok(uid) = run("id", &["-u"]) {
        let _ = run("launchctl", &["kickstart", "-k", &format!("gui/{}/com.mousex.agent", uid.trim())]);
    }
}

/// O systemd encerra junto este `mousex update` (filho do agente); `update` chama isto por último.
#[cfg(target_os = "linux")]
fn start_agent() {
    let _ = run("systemctl", &["--user", "--no-block", "restart", crate::install::SERVICE]);
}

#[cfg(target_os = "linux")]
fn sign(_: &Path, _: &[&str]) {}

/// Reassina com a identidade local do install.sh (mesma ordem dele): assinatura estável mantém a Acessibilidade.
#[cfg(target_os = "macos")]
fn sign(bin: &Path, names: &[&str]) {
    let identities = run("security", &["find-identity", "-p", "codesigning"]).unwrap_or_default();
    let hash = ["Sculpin Local Signing", "MouseX Local Signing"].iter().find_map(|name| {
        let quoted = format!("\"{name}\"");
        identities.lines().find(|line| line.contains(&quoted)).and_then(|line| line.split_whitespace().nth(1))
    });
    let Some(hash) = hash else {
        log("update: no local signing identity; Accessibility may need to be granted again");
        return;
    };
    for name in names {
        let path = bin.join(name).display().to_string();
        if let Err(error) =
            run("codesign", &["--force", "--sign", hash, "--identifier", &format!("com.mousex.{name}"), &path])
        {
            log(&format!("update: signing {name} failed: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mousex-update-{tag}-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).expect("pasta");
        dir
    }

    #[test]
    fn falha_no_meio_volta_so_o_que_foi_trocado_nesta_vez() {
        let (bin, package) = (dir("bin"), dir("pkg"));
        for (name, text) in [("a", "a1"), ("b", "b1"), ("c", "c1"), ("c.old", "c0 de um update anterior")] {
            std::fs::write(bin.join(name), text).expect("bin");
        }
        std::fs::write(package.join("a"), "a2").expect("pkg");
        std::fs::create_dir(package.join("b")).expect("b vira pasta: a cópia falha");
        std::fs::write(package.join("c"), "c2").expect("pkg");
        let (moved, result) = swap(&bin, &package, &["a", "b", "c"]);
        assert!(result.is_err());
        restore(&bin, &moved);
        let read = |name: &str| std::fs::read_to_string(bin.join(name)).expect(name);
        assert_eq!((read("a"), read("b"), read("c")), ("a1".into(), "b1".into(), "c1".into()));
        let _ = (std::fs::remove_dir_all(bin), std::fs::remove_dir_all(package));
    }

    #[test]
    fn troca_completa_guarda_os_anteriores() {
        let (bin, package) = (dir("bin"), dir("pkg"));
        std::fs::write(bin.join("a"), "a1").expect("bin");
        std::fs::write(package.join("a"), "a2").expect("pkg");
        std::fs::write(package.join("novo"), "n2").expect("pkg");
        let (moved, result) = swap(&bin, &package, &["a", "novo"]);
        result.expect("troca");
        assert_eq!(moved, ["a"]);
        assert_eq!(std::fs::read_to_string(bin.join("a")).expect("a"), "a2");
        assert_eq!(std::fs::read_to_string(bin.join("a.old")).expect("a.old"), "a1");
        let _ = (std::fs::remove_dir_all(bin), std::fs::remove_dir_all(package));
    }
}
