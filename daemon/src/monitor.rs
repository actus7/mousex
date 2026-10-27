//! Local status window. The agent serves it on 127.0.0.1:41416; `mousex monitor` opens it.
//! Nothing here injects input. It reads the config, the service, the activity file and the log. On a Linux
//! main computer it also changes the layout, since the Windows screen editor does not run there: the side of
//! the other computer (`POST /api/side`) and the screen editor at `/arrange` (`mousex arrange`).

#[cfg(target_os = "linux")]
use crate::side::{Side, set_side};
use crate::{
    activity::{self, Live},
    config::{Config, Role, data_dir},
};
use mousex_core::i18n::{Lang, lang};
use std::{
    error::Error,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

const PORT: u16 = 41416;
/// The 64 px copy of `assets/mousex.png` that the editor also uses: page headers and the browser tab.
const ICON: &[u8] = include_bytes!("../../app/src/icon.png");

pub fn serve_background() {
    let _ = std::thread::Builder::new().name("mousex-monitor".into()).spawn(|| {
        if let Ok(listener) = bind() {
            serve(listener);
        }
    });
}

pub fn open() -> Result<(), Box<dyn Error>> {
    open_at("/")
}

/// Opens a page of the window in the browser; with no agent answering, this process serves it.
pub fn open_at(path: &str) -> Result<(), Box<dyn Error>> {
    let url = format!("http://127.0.0.1:{PORT}{path}");
    if listening() {
        open_browser(&url)?;
        println!("{url}");
        return Ok(());
    }
    let listener = bind().map_err(|error| format!("monitor port {PORT} is unavailable: {error}"))?;
    if let Err(error) = open_browser(&url) {
        eprintln!("open {url} failed: {error}");
    }
    println!(
        "{}",
        lang().pick(format!("Monitor em {url} — Ctrl+C para fechar"), format!("Monitor at {url} — Ctrl+C to close"))
    );
    serve(listener);
    Ok(())
}

fn bind() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, PORT))
}

fn listening() -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", ""]);
        cmd
    };
    cmd.arg(url).spawn()?;
    Ok(())
}

fn serve(listener: TcpListener) {
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let Some((req, _body)) = read_request(&mut stream) else { continue };
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        if path.starts_with("/favicon") || path == "/icon.png" {
            respond(&mut stream, "image/png", ICON);
            continue;
        }
        #[cfg(target_os = "linux")]
        if let Some((ctype, body)) = linux_route(path, &req, &_body) {
            respond(&mut stream, ctype, body.as_bytes());
            continue;
        }
        let (ctype, body) = if path.starts_with("/api/status") {
            ("application/json; charset=utf-8", status_json())
        } else {
            ("text/html; charset=utf-8", PAGE.to_string())
        };
        respond(&mut stream, ctype, body.as_bytes());
    }
}

/// Request head and body. Only the screen editor sends a body (a layout is a few KB).
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if data.len() > 16 * 1024 {
            return None;
        }
        let n = stream.read(&mut buf).ok().filter(|n| *n > 0)?;
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..end]).into_owned();
    let length = header(&head, "content-length").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    if length > 256 * 1024 {
        return None;
    }
    let mut body = data[end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buf).ok().filter(|n| *n > 0)?;
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(length);
    Some((head, body))
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|line| line.split_once(':').filter(|(key, _)| key.trim().eq_ignore_ascii_case(name)))
        .map(|(_, value)| value.trim())
}

/// Asked as 127.0.0.1 or localhost: another name pointing here (DNS rebinding) gets nothing.
#[cfg(target_os = "linux")]
fn local_host(head: &str) -> bool {
    header(head, "host").is_some_and(|host| host == format!("127.0.0.1:{PORT}") || host == format!("localhost:{PORT}"))
}

/// A POST that changes the layout: from these pages or from outside a browser (no `Origin`), so another site
/// open in the browser cannot change it.
#[cfg(target_os = "linux")]
fn local_post(head: &str) -> bool {
    let ours =
        |origin: &str| origin == format!("http://127.0.0.1:{PORT}") || origin == format!("http://localhost:{PORT}");
    head.starts_with("POST ") && local_host(head) && header(head, "origin").is_none_or(ours)
}

#[cfg(target_os = "linux")]
fn linux_route(path: &str, head: &str, body: &[u8]) -> Option<(&'static str, String)> {
    const JSON: &str = "application/json; charset=utf-8";
    let forbidden = || serde_json::json!({ "ok": false, "message": "forbidden" }).to_string();
    if path.starts_with("/api/side") {
        Some((JSON, side_request(head)))
    } else if path.starts_with("/api/layout") && head.starts_with("GET ") {
        Some((JSON, if local_host(head) { crate::arrange::layout_json() } else { forbidden() }))
    } else if path.starts_with("/api/layout") {
        Some((JSON, crate::arrange::save_request(local_post(head), body)))
    } else if path == "/arrange" || path.starts_with("/arrange?") {
        Some(("text/html; charset=utf-8", crate::arrange::PAGE.to_string()))
    } else {
        None
    }
}

fn respond(stream: &mut TcpStream, ctype: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn status_json() -> String {
    let config = Config::load(&data_dir());
    let log = std::fs::read_to_string(data_dir().join("mousex.log")).unwrap_or_default();
    let status = Status {
        sides: side_items(),
        ..build(config.as_ref(), &service_info(), activity::read().as_ref(), &log, lang())
    };
    serde_json::to_string(&status).unwrap_or_else(|_| "{}".into())
}

struct ServiceInfo {
    state: String,
    pid: String,
    since: String,
}

fn service_info() -> ServiceInfo {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("systemctl");
        cmd.args([
            "--user",
            "show",
            "mousex.service",
            "-p",
            "ActiveState",
            "-p",
            "MainPID",
            "-p",
            "ActiveEnterTimestamp",
        ]);
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
            && let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
        {
            let bus = std::path::PathBuf::from(&dir).join("bus");
            if bus.exists() {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", bus.display()));
            }
        }
        if let Ok(output) = cmd.output()
            && output.status.success()
        {
            return parse_service(&String::from_utf8_lossy(&output.stdout));
        }
    }
    ServiceInfo { state: "unknown".into(), pid: String::new(), since: String::new() }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_service(text: &str) -> ServiceInfo {
    let mut info = ServiceInfo { state: "unknown".into(), pid: String::new(), since: String::new() };
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "ActiveState" => info.state = value.to_string(),
                "MainPID" => info.pid = value.to_string(),
                "ActiveEnterTimestamp" if !value.is_empty() && value != "n/a" => info.since = value.to_string(),
                _ => {}
            }
        }
    }
    info
}

#[derive(serde::Serialize)]
struct Status {
    kind: String,
    pill: String,
    headline: String,
    detail: String,
    machine: String,
    role: String,
    service: String,
    paired: String,
    pointer: String,
    input: String,
    at: u64,
    log_path: String,
    labels: Labels,
    lines: Vec<LogLine>,
    sides: Vec<SideItem>,
}

#[derive(serde::Serialize)]
struct Labels {
    computer: String,
    role: String,
    service: String,
    paired: String,
    now: String,
    log: String,
    pointer: String,
    input: String,
    side: String,
    side_note: String,
    left: String,
    right: String,
    arrange: String,
}

#[derive(serde::Serialize)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct SideItem {
    machine: String,
    /// `left`, `right`, or empty when the computer is not all on one side.
    side: &'static str,
}

#[derive(serde::Serialize)]
struct LogLine {
    sec: Option<u64>,
    text: String,
}

fn build(config: Option<&Config>, service: &ServiceInfo, live: Option<&Live>, log: &str, lang: Lang) -> Status {
    let running = matches!(service.state.as_str(), "active" | "activating");
    let stopped = matches!(service.state.as_str(), "inactive" | "failed" | "deactivating");
    let phase = if stopped {
        "stopped"
    } else if let Some(live) = live {
        live.phase.as_str()
    } else if running && config.is_some_and(|c| c.role == Role::Client) {
        "looking"
    } else if running && config.is_some_and(|c| c.role == Role::Server) {
        "listening"
    } else {
        "idle"
    };
    let detail_raw = live.map(|item| item.detail.as_str()).unwrap_or("");
    let pointer_raw = live.map(|item| item.pointer.as_str()).unwrap_or("");
    let input_raw = live.map(|item| item.input.as_str()).unwrap_or("");
    let (headline, detail) = copy(lang, phase, detail_raw);
    let lines: Vec<&str> = log.lines().collect();
    Status {
        kind: phase.into(),
        pill: pill(lang, phase).into(),
        headline,
        detail,
        machine: config.map(|c| c.machine.clone()).unwrap_or_else(|| "—".into()),
        role: config
            .map(|c| role_name(lang, c.role).to_string())
            .unwrap_or_else(|| lang.pick("não instalado", "not installed").into()),
        service: service_label(lang, service),
        paired: paired(lang, config),
        pointer: format_pointer(lang, pointer_raw),
        input: format_input(lang, input_raw),
        at: live.map(|item| item.at).unwrap_or(0),
        log_path: data_dir().join("mousex.log").display().to_string(),
        labels: Labels {
            computer: lang.pick("Computador", "Computer").into(),
            role: lang.pick("Papel", "Role").into(),
            service: lang.pick("Serviço", "Service").into(),
            paired: lang.pick("Pareado com", "Paired with").into(),
            now: lang.pick("Agora", "Right now").into(),
            log: lang.pick("Log", "Log").into(),
            pointer: lang.pick("Ponteiro", "Pointer").into(),
            input: lang.pick("Última entrada", "Last input").into(),
            side: lang.pick("Lado do outro computador", "Side of the other computer").into(),
            side_note: lang
                .pick(
                    "Padrão: à direita. No terminal: mousex side left ou mousex side right.",
                    "Default: right. From a terminal: mousex side left or mousex side right.",
                )
                .into(),
            left: lang.pick("Esquerda", "Left").into(),
            right: lang.pick("Direita", "Right").into(),
            arrange: lang.pick("Arrumar telas…", "Arrange screens…").into(),
        },
        lines: lines[lines.len().saturating_sub(40)..]
            .iter()
            .map(|line| {
                let (sec, text) = split_log(line);
                LogLine { sec, text: text.to_string() }
            })
            .collect(),
        sides: Vec::new(),
    }
}

/// Linux main computer only: there the side of the other computer is chosen in this window.
#[cfg(target_os = "linux")]
fn side_items() -> Vec<SideItem> {
    crate::side::sides()
        .into_iter()
        .map(|(machine, side)| SideItem { machine, side: side.map_or("", Side::name) })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn side_items() -> Vec<SideItem> {
    Vec::new()
}

/// `POST /api/side?side=left|right&machine=NAME`, the one request that changes something. Accepted from this
/// page or from outside a browser (no `Origin`): another site open in the browser cannot switch the side.
#[cfg(target_os = "linux")]
fn side_request(req: &str) -> String {
    let allowed = local_post(req);
    let query = req.split_whitespace().nth(1).and_then(|path| path.split_once('?')).map_or("", |(_, query)| query);
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.split_once('=').filter(|(key, _)| *key == name))
            .map(|(_, value)| percent_decode(value))
    };
    let result: Result<(String, Side), Box<dyn Error>> = match param("side").as_deref().and_then(Side::parse) {
        _ if !allowed => Err("forbidden".into()),
        Some(side) => {
            set_side(param("machine").as_deref().filter(|name| !name.is_empty()), side).map(|machine| (machine, side))
        }
        None => Err("side must be left or right".into()),
    };
    let lang = lang();
    match result {
        Ok((machine, side)) => {
            let (pt, en) = match side {
                Side::Left => ("à esquerda", "on the left"),
                Side::Right => ("à direita", "on the right"),
            };
            let message = if crate::server::reload_layout() {
                lang.pick(format!("{machine} agora fica {pt}."), format!("{machine} is now {en}."))
            } else {
                lang.pick(
                    format!("{machine} fica {pt} quando o MouseX iniciar."),
                    format!("{machine} goes {en} when MouseX starts."),
                )
            };
            serde_json::json!({ "ok": true, "machine": machine, "message": message }).to_string()
        }
        Err(error) => serde_json::json!({ "ok": false, "message": error.to_string() }).to_string(),
    }
}

/// `mousex side` with the agent running: the agent switches sides itself, with no restart (on GNOME a
/// restart asks again to allow capturing the keyboard and mouse). `None` when no agent answers here.
#[cfg(target_os = "linux")]
pub fn ask_side(machine: Option<&str>, side: Side) -> Option<Result<String, Box<dyn Error>>> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let machine = percent_encode(machine.unwrap_or(""));
    let request = format!(
        "POST /api/side?side={}&machine={machine} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        side.name()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let reply: serde_json::Value = serde_json::from_str(response.split_once("\r\n\r\n")?.1).ok()?;
    let text = |key: &str| reply[key].as_str().unwrap_or_default().to_string();
    Some(if reply["ok"] == true { Ok(text("machine")) } else { Err(text("message").into()) })
}

#[cfg(target_os = "linux")]
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `%XX` from the URL back to bytes: machine names have spaces and accents.
#[cfg(target_os = "linux")]
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = if bytes[i] == b'%' {
            text.get(i + 1..i + 3)
                .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        } else {
            None
        };
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn copy(lang: Lang, phase: &str, detail: &str) -> (String, String) {
    let pair = match phase {
        "stopped" => {
            ("Agente parado", "Agent is stopped", "O serviço não está rodando.", "The service is not running.")
        }
        "rejected" => (
            "Pareamento recusado",
            "Pairing refused",
            "O computador principal não aceitou esta máquina.",
            "The main computer did not accept this one.",
        ),
        "connected" => (
            "Conectado ao computador principal",
            "Connected to the main computer",
            "O teclado e o mouse de lá podem chegar aqui.",
            "Its keyboard and mouse can arrive here.",
        ),
        "listening" => {
            ("Servidor no ar", "Server is up", "Esperando os outros computadores.", "Waiting for the other computers.")
        }
        "looking" => (
            "Procurando o computador principal",
            "Looking for the main computer",
            "Na rede local. Ainda não há um servidor pareado.",
            "On the local network. No server is paired yet.",
        ),
        _ => ("Agente no ar", "Agent is up", "Sem atividade nova.", "No new activity."),
    };
    let headline = lang.pick(pair.0, pair.1).to_string();
    let mut detail_text = lang.pick(pair.2, pair.3).to_string();
    if phase == "connected" && !detail.is_empty() {
        detail_text = lang.pick(format!("Endereço {detail}"), format!("Address {detail}"));
    }
    (headline, detail_text)
}

fn pill(lang: Lang, phase: &str) -> &'static str {
    match phase {
        "stopped" => lang.pick("Parado", "Stopped"),
        "rejected" => lang.pick("Recusado", "Refused"),
        "connected" => lang.pick("Conectado", "Connected"),
        "listening" => lang.pick("Escutando", "Listening"),
        "looking" => lang.pick("Procurando", "Looking"),
        _ => lang.pick("No ar", "Up"),
    }
}

fn role_name(lang: Lang, role: Role) -> &'static str {
    match role {
        Role::Client => lang.pick("Cliente", "Client"),
        Role::Server => lang.pick("Servidor", "Server"),
    }
}

fn paired(lang: Lang, config: Option<&Config>) -> String {
    match config.and_then(|c| c.server_fingerprint.as_deref()) {
        Some(fp) if !fp.is_empty() => fp.chars().take(16).collect(),
        _ => lang.pick("nenhum", "none").into(),
    }
}

fn service_label(lang: Lang, info: &ServiceInfo) -> String {
    let name = match info.state.as_str() {
        "active" => lang.pick("Ativo", "Running"),
        "inactive" => lang.pick("Parado", "Stopped"),
        "failed" => lang.pick("Falhou", "Failed"),
        "activating" => lang.pick("Ligando", "Starting"),
        _ => lang.pick("Sem informação", "Unknown"),
    };
    let mut text = name.to_string();
    if info.state == "active" && !info.pid.is_empty() && info.pid != "0" {
        text.push_str(" · pid ");
        text.push_str(&info.pid);
    }
    if info.state == "active" && !info.since.is_empty() {
        text.push_str(" · ");
        text.push_str(&info.since);
    }
    text
}

fn format_pointer(lang: Lang, raw: &str) -> String {
    if raw.is_empty() {
        return lang.pick("Ainda sem movimento remoto", "No remote pointer yet").into();
    }
    let mut parts = raw.split(':');
    let Some(monitor) = parts.next().and_then(|s| s.parse::<usize>().ok()) else { return raw.into() };
    let (Some(x), Some(y)) = (parts.next(), parts.next()) else { return raw.into() };
    lang.pick(format!("Tela {} · {x}, {y}", monitor + 1), format!("Screen {} · {x}, {y}", monitor + 1))
}

fn format_input(lang: Lang, raw: &str) -> String {
    if raw.is_empty() {
        return lang.pick("Nenhuma tecla ou botão ainda", "No key or button yet").into();
    }
    let parts: Vec<&str> = raw.split(':').collect();
    match parts.as_slice() {
        ["button", "0", down] => button_text(lang, lang.pick("esquerdo", "left"), down),
        ["button", "1", down] => button_text(lang, lang.pick("direito", "right"), down),
        ["button", "2", down] => button_text(lang, lang.pick("do meio", "middle"), down),
        ["button", "3", down] => button_text(lang, lang.pick("voltar", "back"), down),
        ["button", "4", down] => button_text(lang, lang.pick("avançar", "forward"), down),
        ["key", "down"] => lang.pick("Tecla pressionada", "Key pressed").into(),
        ["key", "up"] => lang.pick("Tecla solta", "Key released").into(),
        ["wheel"] => lang.pick("Rolagem", "Scroll").into(),
        ["release"] => lang.pick("Botões e teclas soltos", "Buttons and keys released").into(),
        _ => raw.into(),
    }
}

fn button_text(lang: Lang, name: &str, down: &str) -> String {
    if down == "down" {
        lang.pick(format!("Botão {name} pressionado"), format!("{name} button pressed"))
    } else {
        lang.pick(format!("Botão {name} solto"), format!("{name} button released"))
    }
}

fn split_log(line: &str) -> (Option<u64>, &str) {
    match line.split_once(' ') {
        Some((sec, text)) if sec.chars().all(|c| c.is_ascii_digit()) => (sec.parse().ok(), text),
        _ => (None, line),
    }
}

const PAGE: &str = r##"<!DOCTYPE html>
<html lang="pt">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MouseX</title>
<link rel="icon" href="/icon.png">
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh;
    background: #101218; color: #e8eaef;
    font: 15px/1.45 ui-sans-serif, system-ui, sans-serif;
  }
  main { max-width: 760px; margin: 0 auto; padding: 28px 20px 48px; }
  header { display: flex; justify-content: space-between; align-items: flex-start; gap: 16px; }
  .eyebrow { margin: 0; letter-spacing: .14em; text-transform: uppercase; font-size: 12px; color: #9aa3b2; }
  .eyebrow img { vertical-align: -5px; margin-right: 8px; border-radius: 5px; }
  h1 { margin: 6px 0 0; font-size: 28px; line-height: 1.2; font-weight: 650; }
  h2 { margin: 0 0 10px; font-size: 13px; letter-spacing: .08em; text-transform: uppercase; color: #9aa3b2; font-weight: 600; }
  .pill { border-radius: 999px; padding: 6px 12px; font-size: 13px; font-weight: 650; background: #2a3140; color: #e8eaef; white-space: nowrap; }
  body[data-kind="connected"] .pill { background: #3dd68c; color: #07301c; }
  body[data-kind="looking"] .pill, body[data-kind="listening"] .pill { background: #e6b35a; color: #3a2a08; }
  body[data-kind="rejected"] .pill, body[data-kind="stopped"] .pill { background: #f07178; color: #3b1216; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin: 22px 0; }
  article, .now, .log, .side { background: #181b22; border: 1px solid #2a3140; border-radius: 14px; }
  article { padding: 14px 16px; }
  article span, .meta { display: block; color: #9aa3b2; font-size: 13px; margin-bottom: 4px; }
  article strong, .now p { font-size: 16px; font-weight: 600; }
  .now, .log, .side { padding: 16px; margin-top: 12px; }
  .row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 4px 0 10px; }
  .pick { display: flex; gap: 6px; }
  .pick button { font: inherit; font-size: 14px; font-weight: 600; padding: 6px 14px; border-radius: 999px; border: 1px solid #2a3140; background: #101218; color: #e8eaef; cursor: pointer; }
  .pick button[aria-pressed="true"] { background: #3dd68c; border-color: #3dd68c; color: #07301c; }
  .pick button:disabled { opacity: .55; cursor: default; }
  .link { color: #3dd68c; font-weight: 650; text-decoration: none; }
  .link:hover { text-decoration: underline; }
  .now p { margin: 0 0 8px; font-weight: 500; }
  .now p strong { font-weight: 650; }
  ol { list-style: none; margin: 0; padding: 0; max-height: 320px; overflow: auto; font: 13px/1.55 ui-monospace, monospace; }
  li { display: grid; grid-template-columns: 92px 1fr; gap: 10px; padding: 3px 0; }
  time { color: #9aa3b2; }
  .foot { margin-top: 14px; color: #9aa3b2; font-size: 13px; word-break: break-all; }
  @media (max-width: 640px) { .grid { grid-template-columns: 1fr; } h1 { font-size: 22px; } }
  @media (prefers-reduced-motion: reduce) { * { scroll-behavior: auto; } }
</style>
</head>
<body>
<main>
  <header>
    <div>
      <p class="eyebrow"><img src="/icon.png" alt="" width="20" height="20">MouseX</p>
      <h1 id="headline">…</h1>
    </div>
    <span id="pill" class="pill">…</span>
  </header>
  <section class="grid">
    <article><span id="l-computer">Computador</span><strong id="machine">—</strong></article>
    <article><span id="l-role">Papel</span><strong id="role">—</strong></article>
    <article><span id="l-service">Serviço</span><strong id="service">—</strong></article>
    <article><span id="l-paired">Pareado com</span><strong id="paired">—</strong></article>
  </section>
  <section class="side" id="side" hidden>
    <h2 id="l-side">Lado do outro computador</h2>
    <div id="sides"></div>
    <p class="meta" id="side-msg"></p>
    <p class="meta" id="l-side_note"></p>
    <p><a class="link" id="l-arrange" href="/arrange">Arrumar telas…</a></p>
  </section>
  <section class="now">
    <h2 id="l-now">Agora</h2>
    <p id="detail"></p>
    <p><span class="meta" id="l-pointer">Ponteiro</span><strong id="pointer"></strong></p>
    <p><span class="meta" id="l-input">Última entrada</span><strong id="input"></strong></p>
  </section>
  <section class="log">
    <h2 id="l-log">Log</h2>
    <ol id="log"></ol>
  </section>
  <p class="foot" id="path"></p>
</main>
<script>
const $ = (id) => document.getElementById(id);
function clock(sec) {
  return new Date(sec * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
async function refresh() {
  try {
    const data = await (await fetch("/api/status", { cache: "no-store" })).json();
    document.body.dataset.kind = data.kind || "";
    $("headline").textContent = data.headline || "";
    $("pill").textContent = data.pill || "";
    $("machine").textContent = data.machine || "—";
    $("role").textContent = data.role || "—";
    $("service").textContent = data.service || "—";
    $("paired").textContent = data.paired || "—";
    $("detail").textContent = data.detail || "";
    $("pointer").textContent = data.pointer || "";
    $("input").textContent = data.input || "";
    $("path").textContent = data.log_path || "";
    const labels = data.labels || {};
    for (const key of ["computer", "role", "service", "paired", "now", "log", "pointer", "input", "side", "side_note", "arrange"]) {
      const node = $("l-" + key);
      if (node && labels[key]) node.textContent = labels[key];
    }
    renderSides(data);
    const list = $("log");
    const follow = list.scrollHeight - list.scrollTop - list.clientHeight < 24;
    list.replaceChildren();
    for (const line of data.lines || []) {
      const item = document.createElement("li");
      const time = document.createElement("time");
      time.textContent = line.sec ? clock(line.sec) : "";
      item.append(time, document.createTextNode(line.text || ""));
      list.append(item);
    }
    if (follow) list.scrollTop = list.scrollHeight;
  } catch (error) {
    $("headline").textContent = "Sem resposta";
    $("detail").textContent = String(error);
  }
}
let sidesKey = "";
function renderSides(data) {
  const items = data.sides || [];
  $("side").hidden = items.length === 0;
  const labels = data.labels || {};
  const key = JSON.stringify([items, labels.left, labels.right]);
  if (key === sidesKey) return;
  sidesKey = key;
  const rows = [];
  for (const item of items) {
    const row = document.createElement("div");
    row.className = "row";
    const name = document.createElement("strong");
    name.textContent = item.machine;
    const pick = document.createElement("div");
    pick.className = "pick";
    for (const side of ["left", "right"]) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = labels[side] || side;
      button.setAttribute("aria-pressed", String(item.side === side));
      button.addEventListener("click", () => chooseSide(item, side));
      pick.append(button);
    }
    row.append(name, pick);
    rows.push(row);
  }
  $("sides").replaceChildren(...rows);
}
async function chooseSide(item, side) {
  if (item.side === side) return;
  for (const button of document.querySelectorAll(".pick button")) button.disabled = true;
  try {
    const url = "/api/side?side=" + side + "&machine=" + encodeURIComponent(item.machine);
    const reply = await (await fetch(url, { method: "POST", cache: "no-store" })).json();
    $("side-msg").textContent = reply.message || "";
  } catch (error) {
    $("side-msg").textContent = String(error);
  }
  sidesKey = "";
}
refresh();
setInterval(refresh, 1000);
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn config(role: Role, paired: Option<&str>) -> Config {
        Config {
            role,
            machine: "note".into(),
            server_fingerprint: paired.map(str::to_string),
            remap_modifiers: true,
            auto_update: true,
        }
    }

    fn service(state: &str) -> ServiceInfo {
        ServiceInfo { state: state.into(), pid: "9".into(), since: String::new() }
    }

    #[test]
    fn cliente_ativo_sem_atividade_esta_procurando() {
        let status = build(
            Some(&config(Role::Client, None)),
            &service("active"),
            None,
            "10 client note started, 2 monitor(s)\n",
            Lang::Pt,
        );
        assert_eq!(status.kind, "looking");
        assert_eq!(status.machine, "note");
        assert_eq!(status.paired, "nenhum");
        assert!(status.headline.contains("Procurando"), "{}", status.headline);
        assert_eq!(status.lines.len(), 1);
        assert_eq!(status.lines[0].sec, Some(10));
        assert!(status.lines[0].text.contains("2 monitor"));
    }

    #[test]
    fn servico_parado_vence_o_arquivo_de_atividade() {
        let live = Live {
            phase: "connected".into(),
            detail: "10.0.0.2:41414".into(),
            pointer: "0:3:4".into(),
            input: "button:1:down".into(),
            at: 5,
        };
        let status = build(
            Some(&config(Role::Client, Some("abcdef0123456789ffff"))),
            &service("inactive"),
            Some(&live),
            "",
            Lang::Pt,
        );
        assert_eq!(status.kind, "stopped");
        assert!(status.headline.contains("parado"), "{}", status.headline);
        assert_eq!(status.paired, "abcdef0123456789");
        assert_eq!(status.pointer, "Tela 1 · 3, 4");
        assert!(status.input.contains("direito"), "{}", status.input);
    }

    #[test]
    fn conectado_mostra_o_endereco_e_nao_a_tecla() {
        let live = Live {
            phase: "connected".into(),
            detail: "10.0.0.2:41414".into(),
            pointer: String::new(),
            input: "key:down".into(),
            at: 8,
        };
        let status = build(Some(&config(Role::Client, None)), &service("active"), Some(&live), "", Lang::En);
        assert_eq!(status.kind, "connected");
        assert!(status.detail.contains("10.0.0.2:41414"), "{}", status.detail);
        assert_eq!(status.input, "Key pressed");
        assert_eq!(status.role, "Client");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nome_com_espaco_e_acento_volta_da_url() {
        assert_eq!(percent_encode("MacBook da Júlia"), "MacBook%20da%20J%C3%BAlia");
        assert_eq!(percent_decode("MacBook%20da%20J%C3%BAlia"), "MacBook da Júlia");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%+1"), "%+1");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn outro_site_nao_troca_o_lado() {
        for req in [
            "POST /api/side?side=left HTTP/1.1\r\nHost: 127.0.0.1:41416\r\nOrigin: http://evil.example\r\n\r\n",
            "POST /api/side?side=left HTTP/1.1\r\nHost: evil.example:41416\r\n\r\n",
            "GET /api/side?side=left HTTP/1.1\r\nHost: 127.0.0.1:41416\r\n\r\n",
        ] {
            let body = side_request(req);
            assert!(body.contains("forbidden") && body.contains("false"), "{body}");
        }
    }
}
