//! Instalação no Linux (sem root): binários em `~/.local/share/mousex/bin`, serviço do systemd do usuário
//! (religa o agente se cair), um atalho de início automático da sessão gráfica, que leva DISPLAY e
//! WAYLAND_DISPLAY ao systemd e sobe o serviço, e o MouseX no menu de aplicativos, que abre a janela de status.
//! Os atalhos funcionam em GNOME, KDE, XFCE, Cinnamon…
use crate::{
    client_install::{AGENT, CLI, copy_binaries, print_status, save_role},
    config::{Role, data_dir},
};
use std::{error::Error, path::Path, path::PathBuf, process::Command};

pub const SERVICE: &str = "mousex.service";
const AUTOSTART: &str = "mousex.desktop";
const LAUNCHER: &str = "mousex.desktop";
/// O ícone do menu de aplicativos: `assets/mousex.png` (1024 px), que a área de trabalho reduz.
const ICON: &[u8] = include_bytes!("../../assets/mousex.png");
const ICON_FILE: &str = "mousex.png";
/// Variáveis da sessão gráfica que o serviço precisa e o systemd do usuário nem sempre recebe sozinho.
const SESSION_ENV: &str = "DISPLAY WAYLAND_DISPLAY XAUTHORITY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP";

fn systemctl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("systemctl").arg("--user").args(args).output().map_err(|e| format!("systemctl: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!("systemctl --user {}: {}", args.join(" "), text.trim()))
    }
}

fn config_home() -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::config_dir().ok_or("config folder unavailable")?)
}

fn applications_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::data_dir().ok_or("data folder unavailable")?.join("applications"))
}

fn in_session() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Só as variáveis presentes: sem nenhum nome, o `import-environment` levaria o ambiente inteiro.
fn import_session() -> Result<String, String> {
    let mut import = vec!["import-environment"];
    import.extend(SESSION_ENV.split(' ').filter(|name| std::env::var_os(name).is_some()));
    systemctl(&import)
}

/// O serviço do usuário existe, gravado pelo `mousex install`.
pub fn installed() -> bool {
    config_home().is_ok_and(|dir| dir.join("systemd/user").join(SERVICE).exists())
}

/// Abrir o MouseX (o ícone, `mousex monitor`) liga o serviço instalado e parado. `true` se ligou agora.
pub fn start_if_stopped() -> bool {
    if !installed() || !in_session() || systemctl(&["is-active", "--quiet", SERVICE]).is_ok() {
        return false;
    }
    let _ = import_session();
    systemctl(&["start", SERVICE]).is_ok()
}

/// `--no-block`: quem pede pode ser o próprio agente, que o systemd para no meio do caminho.
pub fn restart_service() -> Result<String, String> {
    systemctl(&["--no-block", "restart", SERVICE])
}

fn service_unit(agent: &Path) -> String {
    format!(
        "[Unit]
Description=MouseX agent: keyboard, mouse and clipboard from the main computer
After=graphical-session.target

[Service]
ExecStart=\"{}\" run
Restart=always
RestartSec=2
",
        escape_exec(&agent.display().to_string())
    )
}

/// Caminho dentro de aspas no `ExecStart`: o systemd trata `%` (especificador), `$` (variável), `\` e `"`.
fn escape_exec(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$")
}

fn autostart_entry() -> String {
    format!(
        "[Desktop Entry]
Type=Application
Name=MouseX
Comment=Keyboard and mouse sharing
Exec=sh -c \"systemctl --user import-environment {SESSION_ENV}; systemctl --user restart {SERVICE}\"
NoDisplay=true
X-GNOME-Autostart-enabled=true
"
    )
}

/// O MouseX no menu de aplicativos. Abrir mostra a janela de status e liga o serviço se estiver parado; o botão
/// direito tem Reiniciar e, no computador principal, Arrumar telas.
fn launcher_entry(cli: &Path, icon: &Path, role: Role) -> String {
    let exec = desktop_exec(&cli.display().to_string());
    let mut entry = format!(
        "[Desktop Entry]
Type=Application
Name=MouseX
Comment=Keyboard and mouse sharing: status window
Comment[pt]=Teclado e mouse compartilhados: janela de status
Exec={exec} monitor
Icon={icon}
Terminal=false
StartupNotify=false
Categories=Utility;
Actions={actions}

[Desktop Action restart]
Name=Restart
Name[pt]=Reiniciar
Exec=systemctl --user restart {SERVICE}
",
        icon = icon.display().to_string().replace('\\', "\\\\"),
        actions = if role == Role::Server { "restart;arrange;" } else { "restart;" },
    );
    if role == Role::Server {
        entry.push_str(&format!(
            "\n[Desktop Action arrange]\nName=Arrange screens\nName[pt]=Arrumar telas\nExec={exec} arrange\n"
        ));
    }
    entry
}

/// Caminho entre aspas no `Exec` de um atalho: `"`, `` ` ``, `$` e `\` levam barra dentro das aspas, e o arquivo
/// dobra cada barra outra vez; `%` é código de campo.
fn desktop_exec(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "linux".into())
}

/// Linux: `--role server|client [--machine NOME]`. O servidor é o computador do teclado e do mouse.
pub fn install(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (role, machine) = parse_args(args, hostname())?;
    systemctl(&["show-environment"]).map_err(|e| format!("MouseX needs systemd's user session: {e}"))?;
    let bin = data_dir().join("bin");
    copy_binaries(&bin)?;
    save_role(role, &machine)?;

    let units = config_home()?.join("systemd/user");
    std::fs::create_dir_all(&units)?;
    std::fs::write(units.join(SERVICE), service_unit(&bin.join(AGENT)))?;
    let autostart = config_home()?.join("autostart");
    std::fs::create_dir_all(&autostart)?;
    std::fs::write(autostart.join(AUTOSTART), autostart_entry())?;
    let icon = data_dir().join(ICON_FILE);
    std::fs::write(&icon, ICON)?;
    let applications = applications_dir()?;
    std::fs::create_dir_all(&applications)?;
    std::fs::write(applications.join(LAUNCHER), launcher_entry(&bin.join(CLI), &icon, role))?;
    systemctl(&["daemon-reload"])?;

    // Instalado de dentro da sessão gráfica: já sobe agora. Por SSH, sobe no próximo login.
    if in_session() {
        import_session()?;
        systemctl(&["restart", SERVICE])?;
    }
    let role_name = if role == Role::Server { "server" } else { "client" };
    println!("MouseX installed as {role_name} ({machine}) in {}.", bin.display());
    println!("Starts automatically at every login. Log: {}", data_dir().join("mousex.log").display());
    println!("MouseX is in the applications menu: it opens the status window and starts MouseX if it is stopped.");
    if role == Role::Server {
        println!("The desktop asks to allow capturing the keyboard and mouse: allow it.");
        println!("GNOME 46 asks again every time MouseX starts (login, update).");
        println!("Other computers install as client on the same network and pair with this one.");
        println!("They start on the right: mousex side left moves them, mousex arrange opens the screen editor.");
    } else {
        println!("On Wayland the system asks once to allow remote control: choose to remember the choice.");
    }
    println!("Watch it live: mousex monitor");
    Ok(())
}

/// `--role server|client [--machine NOME]`.
fn parse_args(args: &[String], default: String) -> Result<(Role, String), Box<dyn Error>> {
    let mut role = None;
    let mut machine = default;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next().map(String::as_str)) {
            ("--role", Some("server")) => role = Some(Role::Server),
            ("--role", Some("client")) => role = Some(Role::Client),
            ("--machine", Some(name)) if !name.trim().is_empty() => machine = name.trim().into(),
            _ => return Err("usage: mousex install --role server|client [--machine NAME]".into()),
        }
    }
    Ok((role.ok_or("missing --role server or --role client")?, machine))
}

pub fn uninstall() -> Result<(), Box<dyn Error>> {
    let _ = systemctl(&["stop", SERVICE]);
    let _ = std::fs::remove_file(config_home()?.join("systemd/user").join(SERVICE));
    let _ = std::fs::remove_file(config_home()?.join("autostart").join(AUTOSTART));
    let _ = std::fs::remove_file(applications_dir()?.join(LAUNCHER));
    let _ = std::fs::remove_file(data_dir().join(ICON_FILE));
    let _ = systemctl(&["daemon-reload"]);
    let _ = std::fs::remove_dir_all(data_dir().join("bin"));
    println!("MouseX removed. Pairing and settings stay in {} (delete it for a clean start).", data_dir().display());
    Ok(())
}

pub fn status() -> Result<(), Box<dyn Error>> {
    let state =
        systemctl(&["show", SERVICE, "--property=ActiveState,SubState,MainPID,ExecMainStatus"]).unwrap_or_else(|e| e);
    print_status(&state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servico_religa_o_agente_e_aceita_caminho_com_espaco() {
        let unit = service_unit(Path::new("/home/ana maria/.local/share/mousex/bin/mousex-agent"));
        assert!(unit.contains("ExecStart=\"/home/ana maria/.local/share/mousex/bin/mousex-agent\" run"), "{unit}");
        assert!(unit.contains("Restart=always"));
    }

    #[test]
    fn caracteres_especiais_do_systemd_no_caminho_sao_escapados() {
        let unit = service_unit(Path::new("/home/a%b$c\"d/mousex-agent"));
        assert!(unit.contains(r#"ExecStart="/home/a%%b$$c\"d/mousex-agent" run"#), "{unit}");
    }

    #[test]
    fn instalacao_aceita_servidor_e_cliente() {
        let (role, machine) =
            parse_args(&["--role".into(), "server".into(), "--machine".into(), " sala ".into()], "pc".into()).unwrap();
        assert_eq!(role, Role::Server);
        assert_eq!(machine, "sala");
        let (role, machine) = parse_args(&["--role".into(), "client".into()], "pc".into()).unwrap();
        assert_eq!(role, Role::Client);
        assert_eq!(machine, "pc");
        assert!(parse_args(&["--role".into(), "foo".into()], "pc".into()).is_err());
    }

    #[test]
    fn atalho_abre_a_janela_e_so_o_principal_arruma_as_telas() {
        let cli = Path::new("/home/ana maria/.local/share/mousex/bin/mousex");
        let icon = Path::new("/home/ana maria/.local/share/mousex/mousex.png");
        let server = launcher_entry(cli, icon, Role::Server);
        assert!(server.contains("\nExec=\"/home/ana maria/.local/share/mousex/bin/mousex\" monitor\n"), "{server}");
        assert!(server.contains("\nIcon=/home/ana maria/.local/share/mousex/mousex.png\n"));
        assert!(server.contains("\nExec=systemctl --user restart mousex.service\n"));
        assert!(server.contains("\" arrange\n") && server.contains("Actions=restart;arrange;\n"));
        assert!(!server.contains("NoDisplay"));
        let client = launcher_entry(cli, icon, Role::Client);
        assert!(client.contains("Actions=restart;\n") && !client.contains("arrange"), "{client}");
    }

    #[test]
    fn caracteres_especiais_no_exec_do_atalho_sao_escapados() {
        assert_eq!(desktop_exec("/home/ana maria/mousex"), r#""/home/ana maria/mousex""#);
        assert_eq!(desktop_exec("/a%b$c\"d`e\\f"), r#""/a%%b\\$c\\"d\\`e\\\\f""#);
    }

    #[test]
    fn inicio_automatico_leva_a_sessao_grafica_ao_systemd() {
        let entry = autostart_entry();
        assert!(entry.contains("import-environment DISPLAY WAYLAND_DISPLAY"), "{entry}");
        assert!(entry.contains("restart mousex.service"));
    }
}
