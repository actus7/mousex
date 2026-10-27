//! Instalação no macOS: binários no Application Support do usuário e LaunchAgent (sem administrador).
use crate::{
    client_install::{AGENT, copy_binaries, print_status, role_and_machine_from_args, save_role},
    config::{Role, data_dir},
};
use std::{error::Error, path::PathBuf, process::Command};

const LABEL: &str = "com.mousex.agent";
/// Serviço de quando o programa se chamava Sculpin.
const LEGACY_LABEL: &str = "com.sculpin.agent";

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

/// `[--role server|client] [--machine NOME]` no Mac (cliente por padrão). O servidor é o computador do teclado
/// e do mouse; os outros se instalam como cliente e pareiam com ele.
pub fn install(args: &[String]) -> Result<(), Box<dyn Error>> {
    let default =
        command("scutil", &["--get", "ComputerName"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| "mac".into());
    let (role, machine) = role_and_machine_from_args(args, default)?;
    migrate_legacy()?;
    let data = data_dir();
    let bin = data.join("bin");
    copy_binaries(&bin)?;
    save_role(role, &machine)?;
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
    let role_name = if role == Role::Server { "server" } else { "client" };
    println!("MouseX installed as {role_name} ({machine}) in {}.", bin.display());
    println!("Starts automatically at every login. Log: {}", data.join("mousex.log").display());
    if role == Role::Server {
        println!(
            "Grant Accessibility and Input Monitoring to \"mousex-agent\" in System Settings > Privacy & Security if macOS asks."
        );
        println!("Other computers install as client on the same network and pair with this one.");
    } else {
        println!(
            "Grant Accessibility to \"mousex-agent\" in System Settings > Privacy & Security > Accessibility if macOS asks."
        );
    }
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
    let state = command("launchctl", &["print", &format!("{}/{LABEL}", domain()?)]).unwrap_or_else(|e| e);
    let service: Vec<&str> = state
        .lines()
        .filter(|l| l.contains("state =") || l.contains("pid =") || l.contains("last exit"))
        .map(str::trim)
        .collect();
    print_status(&service.join("\n"));
    Ok(())
}
