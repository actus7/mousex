//! Instalação: binário em %ProgramFiles%\MouseX, tarefa no logon (privilégio elevado) e firewall.
use crate::config::{Config, Role, data_dir};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub(crate) const TASK: &str = "MouseX";
/// Atualização automática: roda como SYSTEM para gravar em Program Files sem UAC.
const UPDATE_TASK: &str = "MouseX Update";
const FIREWALL_RULE: &str = "MouseX";
const COPY_ATTEMPTS: u32 = 20;
const CLI: &str = "mousex.exe";
const AGENT: &str = "mousex-agent.exe";
const EDITOR: &str = "mousex-editor.exe";
/// O editor (Tauri, toolchain GNU) carrega o WebView2 por esta DLL, que precisa estar ao lado dele.
const EDITOR_DLL: &str = "WebView2Loader.dll";
/// Nome de quando o programa se chamava Sculpin: tarefa, firewall e pastas antigas saem na instalação.
const LEGACY_NAME: &str = "Sculpin";
const REPARSE_POINT: u32 = 0x400;

/// `--role server|client [--machine NOME]`; nome padrão é o do computador.
pub fn parse_install(args: &[String], default_machine: &str) -> Result<(Role, String), String> {
    let mut role = None;
    let mut machine = default_machine.to_string();
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

/// O que o assistente faz: atualizar a instalação existente (mesmo papel e nome) ou perguntar o papel.
#[derive(Debug, PartialEq)]
pub enum SetupPlan {
    Update { role: Role, machine: String },
    Ask { machine: String },
}

pub fn setup_plan(existing: Option<Config>, computer: &str) -> SetupPlan {
    match existing {
        Some(config) => SetupPlan::Update { role: config.role, machine: config.machine },
        None => SetupPlan::Ask { machine: computer.into() },
    }
}

pub(crate) const UNINSTALL_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MouseX";

/// Valores da entrada em Configurações → Aplicativos: (nome, tipo do registro, valor).
fn uninstall_entry(dir: &Path) -> Vec<(&'static str, &'static str, String)> {
    let cli = dir.join(CLI).display().to_string();
    vec![
        ("DisplayName", "REG_SZ", "MouseX".into()),
        ("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION").into()),
        ("Publisher", "REG_SZ", "MouseX".into()),
        ("URLInfoAbout", "REG_SZ", "https://github.com/actus7/mousex".into()),
        ("InstallLocation", "REG_SZ", dir.display().to_string()),
        ("DisplayIcon", "REG_SZ", format!("{cli},0")),
        ("UninstallString", "REG_SZ", format!("\"{cli}\" uninstall")),
        ("NoModify", "REG_DWORD", "1".into()),
        ("NoRepair", "REG_DWORD", "1".into()),
    ]
}

fn register_uninstall(dir: &Path) -> Result<(), String> {
    for (name, kind, value) in uninstall_entry(dir) {
        command("reg", &["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", &value, "/f"])?;
    }
    Ok(())
}

fn is_portuguese() -> bool {
    mousex_core::i18n::lang() == mousex_core::i18n::Lang::Pt
}

/// Duplo clique no `mousex.exe` (ou `mousex setup`): pede administrador, atualiza a instalação existente
/// ou pergunta se este é o computador principal, instala e mostra o resultado numa janela.
pub fn setup() -> Result<(), Box<dyn Error>> {
    use mousex_core::i18n::tr;
    if !mousex_native::is_elevated() {
        // A cópia elevada (UAC) faz o resto; recusar o pedido de administrador só encerra.
        mousex_native::run_elevated(&std::env::current_exe()?, "setup")?;
        return Ok(());
    }
    mousex_native::hide_console();
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "mousex".into());
    let existing = crate::config::system_data_dir().and_then(|dir| Config::load(&dir));
    let (role, machine, updating) = match setup_plan(existing, &computer) {
        SetupPlan::Update { role, machine } => (role, machine, true),
        SetupPlan::Ask { machine } => {
            let main = mousex_native::ask_yes_no(
                "MouseX",
                tr(
                    "Este é o computador com o teclado e o mouse que você usa?\n\nSim: este computador controla os outros (computador principal).\nNão: este computador será controlado por outro.",
                    "Is this the computer whose keyboard and mouse you use?\n\nYes: this computer controls the others (main computer).\nNo: this computer will be controlled by another one.",
                ),
            );
            (if main { Role::Server } else { Role::Client }, machine, false)
        }
    };
    let role_arg = if role == Role::Server { "server" } else { "client" };
    let args: Vec<String> = ["--role", role_arg, "--machine", &machine].iter().map(|s| s.to_string()).collect();
    match install(&args) {
        Ok(()) => {
            let text = match (updating, role, is_portuguese()) {
                (true, _, true) => "O MouseX foi atualizado. Pareamentos e telas continuam como estavam.",
                (true, _, false) => "MouseX was updated. Pairings and screens are unchanged.",
                (false, Role::Server, true) => {
                    "Pronto! Este é o computador principal.\n\nAgora instale o MouseX nos outros computadores. Cada um vai mostrar um código de 6 dígitos aqui e lá: confirme nos dois.\n\nO ícone do MouseX fica perto do relógio."
                }
                (false, Role::Server, false) => {
                    "Done! This is the main computer.\n\nNow install MouseX on the other computers. Each one shows a 6-digit code here and there: confirm on both.\n\nThe MouseX icon lives next to the clock."
                }
                (false, Role::Client, true) => {
                    "Pronto! Quando o computador principal for encontrado, os dois vão mostrar um código de 6 dígitos: confirme nos dois."
                }
                (false, Role::Client, false) => {
                    "Done! When the main computer is found, both will show a 6-digit code: confirm on both."
                }
            };
            mousex_native::show_message("MouseX", text, false);
            Ok(())
        }
        Err(error) => {
            let text =
                format!("{}\n\n{error}", tr("Não foi possível instalar o MouseX:", "MouseX could not be installed:"));
            mousex_native::show_message("MouseX", &text, true);
            Err(error)
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Tarefa no logon, elevada, sem limite de tempo, também na bateria, reiniciando se cair.
pub fn task_xml(exe: &Path, user: &str) -> String {
    let (exe, user) = (escape(&exe.display().to_string()), escape(user));
    // O agente ignora argumentos; "run" fica por legibilidade no Agendador de Tarefas.
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>MouseX: teclado e mouse compartilhados</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
    <RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>run</Arguments></Exec></Actions>
</Task>
"#
    )
}

/// A cada 30 min e 10 min após ligar. Conferir é um único redirecionamento do GitHub (sem API nem limite);
/// só baixa quando há versão nova.
pub fn update_task_xml(cli: &Path) -> String {
    let cli = escape(&cli.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>MouseX: automatic updates (signed releases only)</Description></RegistrationInfo>
  <Triggers>
    <BootTrigger><Enabled>true</Enabled><Delay>PT10M</Delay></BootTrigger>
    <CalendarTrigger><Repetition><Interval>PT30M</Interval><Duration>P1D</Duration></Repetition><StartBoundary>2026-01-01T00:00:00</StartBoundary><Enabled>true</Enabled><ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger>
  </Triggers>
  <Principals><Principal id="System"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>true</RunOnlyIfNetworkAvailable>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT30M</ExecutionTimeLimit>
  </Settings>
  <Actions Context="System"><Exec><Command>{cli}</Command><Arguments>update</Arguments></Exec></Actions>
</Task>
"#
    )
}

/// O update só troca os binários: o agente instalado (elevado) recria a tarefa de atualização ao subir,
/// para ela seguir o agendamento da versão atual e voltar se tiver sido apagada.
pub(crate) fn refresh_update_task() {
    let Ok(dir) = install_dir() else { return };
    let installed = std::env::current_exe().is_ok_and(|exe| exe.parent() == Some(dir.as_path()));
    if !installed {
        return;
    }
    if let Err(error) = create_task(UPDATE_TASK, &update_task_xml(&dir.join(CLI))) {
        crate::log(&format!("update task could not be refreshed: {error}"));
    }
}

/// schtasks lê XML em UTF-16 LE com BOM.
fn create_task(name: &str, xml: &str) -> Result<(), Box<dyn Error>> {
    let path = std::env::temp_dir().join("mousex-task.xml");
    let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    std::fs::write(&path, utf16)?;
    let created = command("schtasks", &["/Create", "/TN", name, "/XML", &path.display().to_string(), "/F"]);
    let _ = std::fs::remove_file(&path);
    created?;
    Ok(())
}

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if output.status.success() { Ok(text) } else { Err(format!("{program} {}: {}", args.join(" "), text.trim())) }
}

/// Program Files (caminho vindo do shell, não do ambiente): só administradores escrevem,
/// então a tarefa elevada nunca roda binário trocado por usuário comum.
fn install_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(mousex_native::program_files_dir()?.join("MouseX"))
}

/// Arquivos levados da pasta antiga do usuário. `config.json` fica de fora: gravá-lo é o último passo da
/// instalação e o que faz `data_dir()` passar a usar a pasta nova (migração pela metade nunca é adotada).
const MIGRATED: &[&str] = &["identity.der", "identity.pk8", "allowlist.json", "layout.json", "last_server.json"];

/// Copia identidade, pareamentos e layout se a pasta nova ainda não tem o par de identidade completo.
/// Depois remove a chave antiga, que ficaria legível por qualquer programa do usuário.
fn migrate_legacy(data: &Path, legacy: &Path) -> std::io::Result<bool> {
    let pair = |dir: &Path| dir.join("identity.der").exists() && dir.join("identity.pk8").exists();
    if pair(data) || !pair(legacy) {
        return Ok(false);
    }
    // A pasta antiga é do usuário: leitura e remoção pelo handle conferido, nunca seguindo links.
    for name in MIGRATED {
        let from = legacy.join(name);
        if !from.exists() {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut mousex_native::open_unlinked(&from, false)?, &mut bytes)?;
        std::fs::write(data.join(name), bytes)?;
    }
    let _ = mousex_native::remove_unlinked(&legacy.join("identity.pk8"));
    Ok(true)
}

/// Pasta protegida de verdade: não é link e pertence a Administradores ou SYSTEM.
fn trusted_dir(dir: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_attributes() & REPARSE_POINT == 0)
        && mousex_native::owned_by_admins(dir).unwrap_or(false)
}

/// Pasta de dados do agente elevado: dona Administradores, só Administradores e SYSTEM escrevem (Usuários leem).
/// Pasta pré-existente criada por usuário comum (ou junction) não é confiável: vai para o lado e recomeça.
/// `legacy`: pastas antigas, na ordem de preferência; a primeira com identidade completa é migrada.
fn secure_data_dir(data: &Path, legacy: &[PathBuf]) -> Result<(), Box<dyn Error>> {
    use std::os::windows::fs::MetadataExt;
    if let Ok(meta) = std::fs::symlink_metadata(data) {
        if meta.file_attributes() & REPARSE_POINT != 0 {
            std::fs::remove_dir(data)?;
        } else if !mousex_native::owned_by_admins(data)? {
            let seconds =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            std::fs::rename(data, data.with_file_name(format!("MouseX.nao-confiavel-{seconds}")))?;
        }
    }
    // Criada já protegida, de uma vez; se existe, só serve a da instalação anterior (outra surgida no meio é recusada).
    match mousex_native::create_protected_dir(data) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !trusted_dir(data) {
                return Err(format!(
                    "{} was created by another program during setup; run the installer again.",
                    data.display()
                )
                .into());
            }
        }
        other => other?,
    }
    let target = data.display().to_string();
    command("icacls", &[&target, "/setowner", "*S-1-5-32-544"])?;
    command(
        "icacls",
        &[
            &target,
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-32-544:(OI)(CI)F",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-545:(OI)(CI)RX",
        ],
    )?;
    for source in legacy {
        if migrate_legacy(data, source)? {
            println!("Dados migrados de {} para {}.", source.display(), data.display());
            break;
        }
    }
    // Valida o par migrado e já restringe a chave privada (sem esperar o agente subir).
    mousex_core::identity::DeviceIdentity::load_or_create(data)?;
    Ok(())
}

pub fn install(args: &[String]) -> Result<(), Box<dyn Error>> {
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "mousex".into());
    let (role, machine) = parse_install(args, &computer)?;
    // whoami dá DOMÍNIO\usuário real; USERDOMAIN vira "WORKGROUP" em sessões remotas.
    let user = command("whoami", &[])?.trim().to_string();
    let dir = install_dir()?;
    std::fs::create_dir_all(&dir)?;
    let exe = dir.join(AGENT);
    for task in [TASK, LEGACY_NAME] {
        let _ = command("schtasks", &["/End", "/TN", task]);
    }
    let _ = command("schtasks", &["/Delete", "/TN", LEGACY_NAME, "/F"]);
    let source_dir = std::env::current_exe()?.parent().map(Path::to_path_buf).ok_or("executable folder unavailable")?;
    for name in [CLI, AGENT, EDITOR, EDITOR_DLL] {
        // O editor é opcional (build separado); sem ele, o menu "Arrumar telas…" só registra no log.
        if matches!(name, EDITOR | EDITOR_DLL) && !source_dir.join(name).exists() {
            continue;
        }
        let (from, to) = (source_dir.join(name), dir.join(name));
        if from == to {
            continue;
        }
        // A instância anterior pode levar alguns instantes para liberar o executável.
        let mut attempt = 0;
        while let Err(error) = std::fs::copy(&from, &to) {
            attempt += 1;
            if attempt >= COPY_ATTEMPTS {
                return Err(format!("could not copy {} to {}: {error}", from.display(), to.display()).into());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    let data = crate::config::system_data_dir().ok_or("ProgramData folder unavailable")?;
    // ProgramData\Sculpin só vale se for protegida como a nova (senão qualquer usuário poderia tê-la criado).
    let old_system = crate::config::legacy_system_data_dir().filter(|dir| trusted_dir(dir));
    let legacy: Vec<PathBuf> = old_system
        .iter()
        .cloned()
        .chain([crate::config::user_data_dir(), crate::config::legacy_user_data_dir()])
        .collect();
    secure_data_dir(&data, &legacy)?;
    let trusted =
        Config::load(&data).or_else(|| old_system.as_deref().and_then(Config::load)).filter(|c| c.role == role);
    // O servidor fixado decide de quem o agente elevado aceita entrada: vem só da pasta protegida, nunca da do usuário.
    let server_fingerprint = trusted.as_ref().and_then(|c| c.server_fingerprint.clone());
    let previous = trusted
        .or_else(|| {
            Config::load(&crate::config::user_data_dir())
                .or_else(|| Config::load(&crate::config::legacy_user_data_dir()))
        })
        .filter(|c| c.role == role);
    let config = Config {
        role,
        machine: machine.clone(),
        server_fingerprint,
        auto_update: previous.as_ref().is_none_or(|c| c.auto_update),
        remap_modifiers: previous.is_none_or(|c| c.remap_modifiers),
    };
    config.save(&data)?;
    create_task(TASK, &task_xml(&exe, &user))?;
    create_task(UPDATE_TASK, &update_task_xml(&dir.join(CLI)))?;
    let _ = command("netsh", &["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")]);
    command(
        "netsh",
        &[
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={FIREWALL_RULE}"),
            "dir=in",
            "action=allow",
            "protocol=UDP",
            "localport=41414,41415",
            "remoteip=localsubnet",
            &format!("program={}", exe.display()),
            "enable=yes",
        ],
    )?;
    command("schtasks", &["/Run", "/TN", TASK])?;
    let _ = command("netsh", &["advfirewall", "firewall", "delete", "rule", &format!("name={LEGACY_NAME}")]);
    // O agente antigo leva alguns instantes para sair e soltar o executável.
    let old_programs = mousex_native::program_files_dir()?.join(LEGACY_NAME);
    for _ in 0..COPY_ATTEMPTS {
        if !old_programs.exists() || std::fs::remove_dir_all(&old_programs).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if let Some(old) = &old_system {
        let _ = std::fs::remove_dir_all(old);
    }
    if let Err(error) = register_uninstall(&dir) {
        crate::log(&format!("Apps & features entry not registered: {error}"));
    }
    println!(
        "MouseX installed as {} ({machine}) in {}.",
        if role == Role::Server { "server" } else { "client" },
        dir.display()
    );
    println!("Starts automatically at every sign-in. Log: {}", data.join("mousex.log").display());
    Ok(())
}

/// Também chamado por Configurações → Aplicativos: pede administrador se preciso e confirma numa janela.
pub fn uninstall() -> Result<(), Box<dyn Error>> {
    use mousex_core::i18n::tr;
    if !mousex_native::is_elevated() {
        mousex_native::run_elevated(&std::env::current_exe()?, "uninstall")?;
        return Ok(());
    }
    let _ = command("schtasks", &["/End", "/TN", TASK]);
    let _ = command("schtasks", &["/Delete", "/TN", TASK, "/F"]);
    let _ = command("schtasks", &["/Delete", "/TN", UPDATE_TASK, "/F"]);
    let _ = command("netsh", &["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")]);
    let _ = command("reg", &["delete", UNINSTALL_KEY, "/f"]);
    std::thread::sleep(Duration::from_millis(500));
    let dir = install_dir()?;
    if std::fs::remove_dir_all(&dir).is_err() {
        // Este executável pode estar rodando de dentro da pasta: ela é apagada logo depois que ele sair.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        let _ = Command::new("cmd").args(["/C", &script]).creation_flags(CREATE_NO_WINDOW).spawn();
    }
    let data = data_dir();
    println!("MouseX removed. Pairings and screen layout stay in {} (delete it for a clean start).", data.display());
    let text = format!(
        "{}\n\n{}",
        tr("O MouseX foi removido deste computador.", "MouseX was removed from this computer."),
        data.display()
    );
    mousex_native::show_message("MouseX", &text, false);
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
    println!("{}", command("schtasks", &["/Query", "/TN", TASK, "/FO", "LIST"]).unwrap_or_else(|e| e));
    if let Ok(log) = std::fs::read_to_string(data_dir().join("mousex.log")) {
        let lines: Vec<&str> = log.lines().collect();
        for line in &lines[lines.len().saturating_sub(15)..] {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tarefa_roda_sempre_elevada_e_sem_limite() {
        let xml = task_xml(Path::new(r"C:\Users\A & B\AppData\Local\MouseX\mousex.exe"), r"PC\usuario");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains(r"<Command>C:\Users\A &amp; B\AppData\Local\MouseX\mousex.exe</Command>"));
        assert!(xml.contains("<Arguments>run</Arguments>"));
        assert!(xml.contains(r"<UserId>PC\usuario</UserId>"));
        assert!(xml.contains("<RestartOnFailure>"));
    }

    #[test]
    fn assistente_atualiza_instalacao_existente_ou_pergunta() {
        assert_eq!(setup_plan(None, "PC"), SetupPlan::Ask { machine: "PC".into() });
        let existing = Config {
            role: Role::Server,
            machine: "sala".into(),
            server_fingerprint: None,
            remap_modifiers: true,
            auto_update: true,
        };
        assert_eq!(setup_plan(Some(existing), "PC"), SetupPlan::Update { role: Role::Server, machine: "sala".into() });
    }

    #[test]
    fn tarefa_de_atualizacao_roda_como_system_e_chama_update() {
        let xml = update_task_xml(Path::new(r"C:\Program Files\MouseX\mousex.exe"));
        assert!(xml.contains("<UserId>S-1-5-18</UserId>"), "SYSTEM grava em Program Files sem UAC");
        assert!(xml.contains(r"<Command>C:\Program Files\MouseX\mousex.exe</Command><Arguments>update</Arguments>"));
        assert!(xml.contains("<Interval>PT30M</Interval>") && xml.contains("<BootTrigger>"));
    }

    #[test]
    fn entrada_em_aplicativos_do_windows() {
        let entry = uninstall_entry(Path::new(r"C:\Program Files\MouseX"));
        let value = |name: &str| entry.iter().find(|(n, _, _)| *n == name).map(|(_, _, v)| v.clone()).expect(name);
        assert_eq!(value("DisplayName"), "MouseX");
        assert_eq!(value("DisplayVersion"), env!("CARGO_PKG_VERSION"));
        assert_eq!(value("UninstallString"), r#""C:\Program Files\MouseX\mousex.exe" uninstall"#);
        assert_eq!(value("DisplayIcon"), r"C:\Program Files\MouseX\mousex.exe,0");
    }

    #[test]
    fn argumentos_de_instalacao() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_install(&args(&["--role", "server"]), "PC").expect("ok"), (Role::Server, "PC".to_string()));
        assert_eq!(
            parse_install(&args(&["--role", "client", "--machine", "win2"]), "PC").expect("ok"),
            (Role::Client, "win2".to_string())
        );
        assert!(parse_install(&args(&["--role", "x"]), "PC").is_err());
        assert!(parse_install(&args(&[]), "PC").is_err());
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mousex-{name}-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).expect("pasta");
        dir
    }

    #[test]
    fn migracao_copia_identidade_e_pareamentos_mas_nao_config() {
        let (legacy, data) = (temp("legado"), temp("dados"));
        for name in ["identity.der", "identity.pk8", "allowlist.json", "layout.json", "config.json", "mousex.log"] {
            std::fs::write(legacy.join(name), name).expect("arquivo");
        }
        assert!(migrate_legacy(&data, &legacy).expect("migração"));
        for name in ["identity.der", "identity.pk8", "allowlist.json", "layout.json"] {
            assert!(data.join(name).exists(), "{name}");
        }
        assert!(!data.join("config.json").exists(), "config é o marcador de conclusão: gravado só no fim");
        assert!(!data.join("mousex.log").exists());
        assert!(!legacy.join("identity.pk8").exists(), "chave antiga removida");
    }

    #[test]
    fn migracao_recusa_pasta_antiga_que_e_junction() {
        let (real, data) = (temp("alvo"), temp("dados"));
        for name in ["identity.der", "identity.pk8"] {
            std::fs::write(real.join(name), name).expect("arquivo");
        }
        let legacy = real.with_file_name(format!("mousex-junction-{}", rand::random::<u64>()));
        command("cmd", &["/C", "mklink", "/J", &legacy.display().to_string(), &real.display().to_string()])
            .expect("junction");
        assert!(
            migrate_legacy(&data, &legacy).is_err(),
            "junction apontaria o agente elevado para arquivos de outro lugar"
        );
        assert!(!data.join("identity.pk8").exists());
        assert!(real.join("identity.pk8").exists(), "nada apagado através da junction");
        let _ = std::fs::remove_dir(&legacy);
    }

    #[test]
    fn migracao_pela_metade_e_refeita() {
        let (legacy, data) = (temp("legado"), temp("dados"));
        for name in ["identity.der", "identity.pk8"] {
            std::fs::write(legacy.join(name), name).expect("arquivo");
        }
        std::fs::write(data.join("identity.der"), "parcial").expect("parcial");
        assert!(migrate_legacy(&data, &legacy).expect("migração"));
        assert_eq!(std::fs::read_to_string(data.join("identity.pk8")).expect("chave"), "identity.pk8");
        assert!(!migrate_legacy(&data, &legacy).expect("nada a fazer"), "par completo não migra de novo");
    }
}
