//! Diálogo, aviso, bloqueio e "usuário ativo" pelas ferramentas que GNOME, KDE e afins já trazem.
use super::x11::X11;
use std::{
    io,
    process::{Command, Stdio},
};

fn quiet(program: &str, args: &[&str]) -> io::Result<bool> {
    let status = Command::new(program).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
    Ok(status.success())
}

/// Sim/não pelo zenity (GNOME e a maioria) ou kdialog (KDE). Os dois saem com 0 para sim e 1 para não;
/// outro código (sem tela, opção desconhecida) ou programa ausente passa para o próximo.
pub fn ask_yes_no(title: &str, text: &str) -> io::Result<bool> {
    let attempts: [(&str, &[&str]); 2] = [
        ("zenity", &["--question", "--no-wrap", "--no-markup", "--title", title, "--text", text]),
        ("kdialog", &["--title", title, "--yesno", text]),
    ];
    let mut failures = Vec::new();
    for (program, args) in attempts {
        match Command::new(program).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
            Ok(status) if matches!(status.code(), Some(0 | 1)) => return Ok(status.success()),
            Ok(status) => failures.push(format!("{program} exited with {status}")),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }
    Err(io::Error::other(format!("no dialog program worked ({}); install zenity or kdialog", failures.join(", "))))
}

pub fn notify(title: &str, text: &str) {
    let _ = quiet("notify-send", &["--app-name=MouseX", title, text]);
}

/// Bloqueio pelo logind (GNOME, KDE, Cinnamon, XFCE com light-locker…); sem ele, pelo xdg-screensaver.
pub fn lock_screen() -> io::Result<()> {
    if quiet("loginctl", &["lock-session"]).unwrap_or(false) || quiet("xdg-screensaver", &["lock"])? {
        Ok(())
    } else {
        Err(io::Error::other("no screen locker answered"))
    }
}

/// Usuário ativo em outra máquina: zera a ociosidade do X11 e avisa o protetor de tela pelo D-Bus.
// ponytail: o GNOME no Wayland não implementa SimulateUserActivity; lá a tela pode apagar enquanto se usa
// a outra máquina. Segurar um Inhibit (portal) durante a atividade se isso incomodar.
pub fn keep_awake() {
    if let Ok(x11) = X11::connect() {
        let _ = x11.reset_idle();
    }
    let _ = quiet(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.ScreenSaver",
            "--object-path",
            "/org/freedesktop/ScreenSaver",
            "--method",
            "org.freedesktop.ScreenSaver.SimulateUserActivity",
        ],
    );
}
