//! Instalação no macOS: binários no Application Support do usuário e LaunchAgent (sem administrador).
use crate::config::{Config, Role, data_dir};
use std::{error::Error, path::PathBuf, process::Command};

const LABEL: &str = "com.mousex.agent";
/// Serviço de quando o programa se chamava Sculpin.
const LEGACY_LABEL: &str = "com.sculpin.agent";
const CLI: &str = "mousex";
const AGENT: &str = "mousex-agent";

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if output.status.success() { Ok(text) } else { Err(format!("{program} {}: {}", args.join(" "), text.trim())) }
}

fn plist_path() -> Result<PathBuf, Box<dyn Error>> {
    plist_for(LABEL)
}

fn plist_for(label: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::home_dir().ok_or("home folder unavailable")?.join("Library/LaunchAgents").join(format!("{label}.plist")))
}

/// Instalação antiga (Sculpin): o serviço sai e a pasta de dados (pareamento, aviso de ABNT2) vira a nova.
fn migrate_legacy() -> Result<(), Box<dyn Error>> {
    let _ = command("launchctl", &["bootout", &format!("{}/{LEGACY_LABEL}", domain()?)]);
    let _ = std::fs::remove_file(plist_for(LEGACY_LABEL)?);
    let (old, data) = (crate::config::legacy_user_data_dir(), data_dir());
    if old.exists() && !data.exists() {
        std::fs::rename(&old, &data)?;
        // Binários antigos: os novos são copiados logo em seguida.
        let _ = std::fs::remove_dir_all(data.join("bin"));
        println!("Data migrated from {} to {}.", old.display(), data.display());
    }
    Ok(())
}

fn domain() -> Result<String, Box<dyn Error>> {
    Ok(format!("gui/{}", command("id", &["-u"])?.trim()))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Cliente no Mac: `--role client [--machine NOME]`.
pub fn install(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut machine =
        command("scutil", &["--get", "ComputerName"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| "mac".into());
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next().map(String::as_str)) {
            ("--role", Some("client")) => {}
            ("--role", Some(_)) => return Err("on macOS only the client role is available".into()),
            ("--machine", Some(name)) if !name.trim().is_empty() => machine = name.trim().into(),
            _ => return Err("uso: mousex install --role client [--machine NOME]".into()),
        }
    }
    migrate_legacy()?;
    let data = data_dir();
    let bin = data.join("bin");
    std::fs::create_dir_all(&bin)?;
    let source = std::env::current_exe()?.parent().map(PathBuf::from).ok_or("executable folder unavailable")?;
    for name in [CLI, AGENT] {
        let (from, to) = (source.join(name), bin.join(name));
        if from != to {
            // Remover antes de copiar: sobrescrever um binário assinado em uso invalida a assinatura em cache.
            let _ = std::fs::remove_file(&to);
            std::fs::copy(&from, &to)?;
        }
    }
    let previous = Config::load(&data).filter(|c| c.role == Role::Client);
    let config = Config {
        role: Role::Client,
        machine: machine.clone(),
        server_fingerprint: previous.as_ref().and_then(|c| c.server_fingerprint.clone()),
        remap_modifiers: previous.is_none_or(|c| c.remap_modifiers),
    };
    config.save(&data)?;
    let agent = escape(&bin.join(AGENT).display().to_string());
    let errors = escape(&data.join("agent.err").display().to_string());
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{agent}</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardErrorPath</key><string>{errors}</string>
</dict>
</plist>
"#
    );
    let path = plist_path()?;
    std::fs::create_dir_all(path.parent().ok_or("LaunchAgents folder unavailable")?)?;
    std::fs::write(&path, plist)?;
    let domain = domain()?;
    let _ = command("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
    command("launchctl", &["bootstrap", &domain, &path.display().to_string()])?;
    println!("MouseX installed as client ({machine}) in {}.", bin.display());
    println!("Starts automatically at every login. Log: {}", data.join("mousex.log").display());
    println!(
        "Grant Accessibility to \"mousex-agent\" in System Settings > Privacy & Security > Accessibility if macOS asks."
    );
    Ok(())
}

pub fn uninstall() -> Result<(), Box<dyn Error>> {
    let _ = command("launchctl", &["bootout", &format!("{}/{LABEL}", domain()?)]);
    let _ = std::fs::remove_file(plist_path()?);
    let _ = std::fs::remove_dir_all(data_dir().join("bin"));
    println!("MouseX removed. Pairing and settings stay in {} (delete it for a clean start).", data_dir().display());
    Ok(())
}

pub fn status() -> Result<(), Box<dyn Error>> {
    match Config::load(&data_dir()) {
        Some(config) => println!(
            "role: {:?}, computer: {}, paired server: {}",
            config.role,
            config.machine,
            config.server_fingerprint.as_deref().map_or("none", |fp| &fp[..16.min(fp.len())])
        ),
        None => println!("not configured"),
    }
    let state = command("launchctl", &["print", &format!("{}/{LABEL}", domain()?)]).unwrap_or_else(|e| e);
    for line in state.lines().filter(|l| l.contains("state =") || l.contains("pid =") || l.contains("last exit")) {
        println!("{}", line.trim());
    }
    if let Ok(log) = std::fs::read_to_string(data_dir().join("mousex.log")) {
        let lines: Vec<&str> = log.lines().collect();
        for line in &lines[lines.len().saturating_sub(15)..] {
            println!("{line}");
        }
    }
    Ok(())
}
