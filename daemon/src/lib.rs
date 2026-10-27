mod activity;
#[cfg(target_os = "linux")]
mod arrange;
pub(crate) mod client;
#[cfg(not(windows))]
pub(crate) mod client_install;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) mod client_tray;
pub(crate) mod clip;
pub(crate) mod config;
pub(crate) mod link_status;
// Só o servidor (Windows) arruma as telas.
#[cfg(windows)]
pub(crate) mod install;
#[cfg(target_os = "macos")]
#[path = "mac_install.rs"]
pub(crate) mod install;
#[cfg(target_os = "linux")]
#[path = "linux_install.rs"]
pub(crate) mod install;
#[cfg(any(windows, target_os = "linux"))]
pub(crate) mod layout;
#[cfg(target_os = "linux")]
mod linux_tray;
mod monitor;
#[cfg(any(windows, target_os = "linux"))]
mod os;
#[cfg(windows)]
pub(crate) mod platform;
#[cfg(target_os = "macos")]
#[path = "mac_platform.rs"]
pub(crate) mod platform;
#[cfg(target_os = "linux")]
#[path = "linux_platform.rs"]
pub(crate) mod platform;
#[cfg(any(windows, target_os = "linux"))]
pub(crate) mod server;
#[cfg(target_os = "linux")]
mod side;
#[cfg(windows)]
mod supervisor;
pub(crate) mod update;

use config::{Config, Role, data_dir};
use mousex_core::{
    discovery::{self, ANY},
    identity::DeviceIdentity,
    net,
    pairing::pairing_code,
    proto::Control,
};
use std::{
    error::Error,
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LOG_MAX_BYTES: u64 = 1024 * 1024;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Log em arquivo com rotação simples (um `.old`). Nunca chamado pelo caminho dos hooks.
pub fn log(line: &str) {
    let path = data_dir().join("mousex.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{seconds} {line}");
    }
}

const USAGE: &str = "MouseX — share one keyboard, mouse and clipboard across computers

usage:
  mousex setup                                          guided install (Windows; same as double-clicking mousex.exe)
  mousex install --role server|client [--machine NAME]  install or update
  mousex uninstall                                      remove (pairings and layout are kept)
  mousex status                                         show configuration, service state and recent log
  mousex monitor                                        open the live status window
  mousex arrange                                        Linux main computer: open the screen editor
  mousex side left|right [MACHINE]                      Linux main computer: put the other computer on that side
  mousex update                                        install the newest signed release now, if any
  mousex version                                        print the version
  mousex run                                            run the agent in the foreground";

/// `mousex.exe`: comandos de terminal (console normal, saída visível, o terminal espera).
pub fn cli_main() {
    let _ = platform::prepare();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(),
        Some("install") => install::install(&args[1..]),
        Some("uninstall") => install::uninstall(),
        Some("status") => install::status(),
        Some("monitor") => monitor::open(),
        #[cfg(target_os = "linux")]
        Some("arrange") => monitor::open_at("/arrange"),
        #[cfg(target_os = "linux")]
        Some("side") => side::cli(&args[1..]),
        Some("update") => update::update(),
        Some("version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // Duplo clique no Explorer (sem argumentos) abre o assistente de instalação.
        #[cfg(windows)]
        None | Some("setup") => install::setup(),
        _ => Err(USAGE.into()),
    };
    if let Err(error) = result {
        log(&format!("error: {error}"));
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

/// `mousex-agent.exe`: o serviço em segundo plano iniciado pela tarefa no logon, sem janela.
pub fn agent_main() {
    // A tarefa inicia o vigia; ele inicia o agente de verdade com `worker` e o religa se cair.
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() != Some(supervisor::WORKER) {
        install::refresh_update_task();
        return supervisor::supervise();
    }
    if !platform::prepare() {
        log("warning: platform setup incomplete (DPI on Windows, Accessibility on Mac, no graphical session on Linux)");
    }
    #[cfg(target_os = "linux")]
    arm_run_limit();
    #[cfg(not(windows))]
    update::schedule();
    monitor::serve_background();
    // O AppKit só mostra o ícone da barra de menus na thread principal, então o agente vai para outra.
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(run_or_exit);
        client_tray::run_menu_bar();
    }
    #[cfg(not(target_os = "macos"))]
    run_or_exit();
}

fn run_or_exit() {
    if let Err(error) = run() {
        log(&format!("fatal error: {error}"));
        std::process::exit(1);
    }
}

/// One next Linux start only. The marker is removed as the limit is armed, and
/// `systemctl stop` ends the service without Restart=always bringing it back.
#[cfg(target_os = "linux")]
fn arm_run_limit() {
    let marker = data_dir().join("stop-after-5m");
    if std::fs::remove_file(&marker).is_err() {
        return;
    }
    log("this run stops automatically after 5 minutes");
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5 * 60));
        log("5 minute run limit reached; stopping the agent");
        let _ = std::process::Command::new("systemctl").args(["--user", "stop", "mousex.service"]).status();
        std::process::exit(0);
    });
}

fn run() -> Result<(), Box<dyn Error>> {
    activity::enable();
    let dir = data_dir();
    let config = Config::load(&dir).ok_or("configuração ausente; rode mousex install")?;
    let identity = DeviceIdentity::load_or_create(&dir)?;
    // Dois workers bastam: o trabalho é pequeno e menos threads é menos memória.
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    runtime.block_on(async move {
        match config.role {
            #[cfg(any(windows, target_os = "linux"))]
            Role::Server => server::run(config.machine, identity, dir).await.map_err(Into::into),
            #[cfg(not(any(windows, target_os = "linux")))]
            Role::Server => Err("the server role is only available on Windows and Linux".into()),
            Role::Client => run_client(config, identity, dir).await,
        }
    })
}

async fn run_client(config: Config, identity: DeviceIdentity, dir: std::path::PathBuf) -> Result<(), Box<dyn Error>> {
    let endpoint = net::client_endpoint(&identity)?;
    let monitors = platform::local_monitors();
    let server_fp = Arc::new(Mutex::new(config.server_fingerprint.clone()));
    let cache = dir.join("last_server.json");
    log(&format!("client {} started, {} monitor(s)", config.machine, monitors.len()));
    #[cfg(target_os = "linux")]
    client_tray::start().await;

    let locate = {
        let server_fp = server_fp.clone();
        move || {
            let wanted = server_fp.lock().map(|fp| fp.clone()).ok().flatten().unwrap_or_else(|| ANY.into());
            let cache = cache.clone();
            async move { discovery::find(&wanted, &cache, DISCOVERY_TIMEOUT).await }
        }
    };
    let authorize = {
        let own = identity.fingerprint.clone();
        move |peer: String, secret: [u8; 32]| {
            let (server_fp, dir, own) = (server_fp.clone(), dir.clone(), own.clone());
            async move {
                let known = server_fp.lock().map(|fp| fp.clone()).ok().flatten();
                if let Some(known) = known {
                    return peer == known;
                }
                let code = pairing_code(&secret, &own);
                let text = mousex_core::i18n::lang().pick(
                    format!("Usar o teclado e o mouse do computador principal do MouseX encontrado na rede?\n\nCódigo: {code:06}\n\nConfirme somente se o mesmo código aparece no computador principal."),
                    format!("Use the keyboard and mouse of the MouseX main computer found on the network?\n\nCode: {code:06}\n\nConfirm only if the same code is shown on the main computer."),
                );
                let title = mousex_core::i18n::tr("MouseX — pareamento", "MouseX — pairing");
                let accepted =
                    tokio::task::spawn_blocking(move || platform::ask_yes_no(title, &text)).await.unwrap_or(false);
                if accepted {
                    if let Ok(mut fp) = server_fp.lock() {
                        *fp = Some(peer.clone());
                    }
                    if let Some(mut saved) = Config::load(&dir) {
                        saved.server_fingerprint = Some(peer);
                        if let Err(error) = saved.save(&dir) {
                            log(&format!("failed to save pairing: {error}"));
                        }
                    }
                }
                accepted
            }
        }
    };
    let hello = {
        // Monitores lidos a cada conexão: telas rearranjadas enquanto desconectado também valem.
        let machine = config.machine.clone();
        move || Control::Hello { machine: machine.clone(), monitors: platform::local_monitors() }
    };
    client::client_loop(endpoint, locate, authorize, hello, platform::Native::new(&monitors)).await;
    Ok(())
}
