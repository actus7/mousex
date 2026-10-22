//! `mousex update`: baixa a release mais nova, confere assinatura e hash, troca os binários e volta
//! atrás se o novo não abrir. Windows: tarefa diária como SYSTEM. Mac: o agente chama a cada 24 h.
//! `curl` e `tar` são os do sistema (Windows 10+ e macOS já trazem os dois).
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
#[cfg(target_os = "macos")]
const FILES: &[&str] = &["mousex", "mousex-agent"];
#[cfg(target_os = "macos")]
const CLI: &str = "mousex";
#[cfg(target_os = "macos")]
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
    let result = download_and_verify(&tag, &work).and_then(|()| replace(&bin, &work.join("package"), &tag));
    let _ = std::fs::remove_dir_all(&work);
    match &result {
        Ok(()) => log(&format!("update: now running {tag}")),
        Err(error) => log(&format!("update to {tag} failed: {error}")),
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
    stop_agent();
    let names: Vec<&str> = FILES.iter().copied().filter(|name| package.join(name).exists()).collect();
    let old = |name: &str| bin.join(format!("{name}.old"));
    let swapped = names.iter().try_for_each(|name| {
        let _ = std::fs::remove_file(old(name));
        if bin.join(name).exists() {
            // Renomear funciona com o executável em uso (Windows) e preserva a assinatura em cache (Mac).
            std::fs::rename(bin.join(name), old(name))?;
        }
        std::fs::copy(package.join(name), bin.join(name)).map(drop)
    });
    let healthy = swapped.map_err(Box::<dyn Error>::from).and_then(|()| {
        sign(bin, &names);
        let version = run(&bin.join(CLI).display().to_string(), &["version"])?;
        if version.trim() == tag.trim_start_matches('v') {
            Ok(())
        } else {
            Err(format!("new binary reports version {:?}", version.trim()).into())
        }
    });
    if healthy.is_ok() {
        record_version(tag);
    }
    if healthy.is_err() {
        for name in &names {
            if old(name).exists() {
                let _ = std::fs::remove_file(bin.join(name));
                let _ = std::fs::rename(old(name), bin.join(name));
            }
        }
    }
    start_agent();
    healthy
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
fn stop_agent() {
    let _ = run("schtasks", &["/End", "/TN", crate::install::TASK]);
}

/// Sem ninguém logado a tarefa não sobe agora; ela sobe sozinha no próximo logon.
#[cfg(windows)]
fn start_agent() {
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

#[cfg(target_os = "macos")]
fn record_version(_: &str) {}

/// Mac: o agente roda `mousex update` 10 min depois de subir e depois a cada 24 h.
// ponytail: o sleep pausa com o Mac dormindo, então "24 h" pode virar mais; o reinício diário do agente
// (logon) compensa. Agendar pelo launchd (StartCalendarInterval) se isso atrasar demais.
#[cfg(target_os = "macos")]
pub fn schedule() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(600));
        loop {
            if let Ok(dir) = bin_dir() {
                let _ = Command::new(dir.join(CLI)).arg("update").status();
            }
            std::thread::sleep(std::time::Duration::from_secs(86_400));
        }
    });
}

#[cfg(target_os = "macos")]
fn bin_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(data_dir().join("bin"))
}

/// No Mac o agente é quem chamou o update: ele só reinicia no fim, por `launchctl kickstart -k`.
#[cfg(target_os = "macos")]
fn stop_agent() {}

#[cfg(target_os = "macos")]
fn start_agent() {
    if let Ok(uid) = run("id", &["-u"]) {
        let _ = run("launchctl", &["kickstart", "-k", &format!("gui/{}/com.mousex.agent", uid.trim())]);
    }
}

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
