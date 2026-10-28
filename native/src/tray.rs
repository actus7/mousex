//! Ícone na bandeja: status das máquinas, pausar, prender cursor, reconectar, "Arrumar telas…", "Abrir log"
//! e, por último, a versão instalada, que abre o "Sobre".
//! Também avisa mudança de monitores (WM_DISPLAYCHANGE) e bloqueio da sessão (WM_WTSSESSION_CHANGE).
//! Janela oculta de nível superior numa thread própria; eventos saem pelo mesmo canal dos hooks.
use super::{publish, Event, TrayCommand};
use std::{
    io, ptr,
    sync::{
        atomic::{AtomicPtr, AtomicU32, Ordering},
        mpsc, Mutex,
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW,
        RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION},
    },
    UI::{Shell::*, WindowsAndMessaging::*},
};

const WM_TRAY: u32 = WM_APP + 1;
/// Status novo publicado por outra thread: o ícone é trocado na thread da janela.
const WM_STATUS: u32 = WM_APP + 2;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const WTS_SESSION_LOCK: usize = 7;
const MENU_ARRANGE: usize = 1;
const MENU_LOG: usize = 2;
const MENU_PAUSE: usize = 3;
const MENU_LOCK: usize = 4;
const MENU_RECONNECT: usize = 5;
/// Último item: a versão instalada; abre o "Sobre".
const MENU_ABOUT: usize = 6;
/// Máquina N do menu: MENU_JUMP + N.
const MENU_JUMP: usize = 100;
/// Recursos de ícone embutidos pelo build: 1 = ativo, 2 = cinza (nenhuma máquina conectada ou pausado).
const ICON_ACTIVE: usize = 1;
const ICON_IDLE: usize = 2;
const SEPARATOR: &str = "";
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static WINDOW: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(ptr::null_mut());
static STATUS: Mutex<TrayStatus> =
    Mutex::new(TrayStatus { machines: Vec::new(), active: false, paused: false, locked: false, portuguese: false });

/// O que o menu e o ícone mostram; o servidor publica a cada mudança.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayStatus {
    /// Máquinas da esquerda para a direita (numeração dos atalhos), se estão conectadas e a versão do MouseX delas.
    pub machines: Vec<(String, bool, Option<String>)>,
    /// Alguma máquina remota conectada e compartilhamento ligado: ícone colorido.
    pub active: bool,
    pub paused: bool,
    pub locked: bool,
    /// Textos em português (senão inglês).
    pub portuguese: bool,
}

pub fn set_tray_status(status: TrayStatus) {
    if let Ok(mut current) = STATUS.lock() {
        *current = status;
    }
    let hwnd = WINDOW.load(Ordering::Acquire);
    // SAFETY: mensagem própria para a janela da bandeja (ou nada, se ela ainda não existe).
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, WM_STATUS, 0, 0) };
    }
}

fn status() -> TrayStatus {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Itens do menu: (id, texto, flags). Separador tem texto vazio.
fn menu_items(status: &TrayStatus) -> Vec<(usize, String, u32)> {
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    let t = |pt: &str, en: &str| if status.portuguese { pt.to_string() } else { en.to_string() };
    let mut items: Vec<(usize, String, u32)> = status
        .machines
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, (name, connected, version))| {
            let mark = if *connected { "●" } else { "○" };
            let flags = if *connected && !status.paused { MF_STRING } else { MF_STRING | MF_GRAYED };
            // Depois do \t o Windows alinha à direita: atalho e, se o cliente informou, a versão dele.
            let version = version.as_deref().map(|v| format!("    v{v}")).unwrap_or_default();
            (MENU_JUMP + i + 1, format!("{mark}  {name}\tCtrl+Alt+Shift+{}{version}", i + 1), flags)
        })
        .collect();
    if !items.is_empty() {
        items.push((0, SEPARATOR.into(), MF_SEPARATOR));
    }
    items.extend([
        (MENU_PAUSE, t("Pausar compartilhamento", "Pause sharing"), MF_STRING | check(status.paused)),
        (
            MENU_LOCK,
            t("Prender cursor na tela\tScroll Lock", "Lock cursor to screen\tScroll Lock"),
            MF_STRING | check(status.locked),
        ),
        (MENU_RECONNECT, t("Reconectar máquinas", "Reconnect computers"), MF_STRING),
        (0, SEPARATOR.into(), MF_SEPARATOR),
        (MENU_ARRANGE, t("Arrumar telas…", "Arrange screens…"), MF_STRING),
        (MENU_LOG, t("Abrir log", "Open log"), MF_STRING),
        (0, SEPARATOR.into(), MF_SEPARATOR),
        (MENU_ABOUT, format!("MouseX {}", env!("CARGO_PKG_VERSION")), MF_STRING),
    ]);
    items
}

fn about_text(portuguese: bool) -> String {
    mousex_core::about::about_text(
        portuguese,
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_LICENSE"),
        env!("CARGO_PKG_REPOSITORY"),
    )
}

fn tooltip(status: &TrayStatus) -> String {
    let t = |pt: &'static str, en: &'static str| if status.portuguese { pt } else { en };
    let connected: Vec<&str> = status.machines.iter().filter(|(_, c, _)| *c).map(|(n, _, _)| n.as_str()).collect();
    let state = if status.paused {
        t(" (pausado)", " (paused)")
    } else if status.locked {
        t(" (cursor preso)", " (cursor locked)")
    } else {
        ""
    };
    if status.active {
        format!("MouseX{state} — {}", connected.join(", "))
    } else {
        format!("MouseX{state} — {}", t("nenhuma outra máquina conectada", "no other computer connected"))
    }
}

fn command_for(id: usize) -> Option<TrayCommand> {
    Some(match id {
        MENU_ARRANGE => TrayCommand::Arrange,
        MENU_LOG => TrayCommand::ShowLog,
        MENU_PAUSE => TrayCommand::Pause,
        MENU_LOCK => TrayCommand::LockCursor,
        MENU_RECONNECT => TrayCommand::Reconnect,
        n if n > MENU_JUMP && n <= MENU_JUMP + 9 => TrayCommand::Jump((n - MENU_JUMP) as u8),
        _ => return None,
    })
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: estrutura POD; campos usados preenchidos abaixo.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data
}

/// Ícone `id` embutido no executável (compartilhado: não é liberado); sem ele (build sem windres), o padrão do sistema.
fn load_icon(id: usize) -> HICON {
    // SAFETY: recurso do próprio módulo; LR_SHARED dispensa DestroyIcon.
    unsafe {
        let size = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let own = LoadImageW(GetModuleHandleW(ptr::null()), id as _, IMAGE_ICON, size.0, size.1, LR_SHARED);
        if own.is_null() {
            LoadIconW(ptr::null_mut(), IDI_APPLICATION)
        } else {
            own as HICON
        }
    }
}

/// Ícone e dica conforme o status atual; `NIM_ADD` retorna `false` se a barra de tarefas ainda não existe (logon).
fn apply_icon(hwnd: HWND, operation: NOTIFY_ICON_MESSAGE) -> bool {
    let current = status();
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = load_icon(if current.active && !current.paused { ICON_ACTIVE } else { ICON_IDLE });
    let tip: Vec<u16> = tooltip(&current).encode_utf16().collect();
    let length = tip.len().min(data.szTip.len() - 1);
    data.szTip[..length].copy_from_slice(&tip[..length]);
    // SAFETY: estrutura válida e janela viva.
    unsafe { Shell_NotifyIconW(operation, &data) != 0 }
}

const RETRY_TIMER: usize = 1;
const RETRY_MS: u32 = 2000;

/// Adiciona o ícone ou agenda nova tentativa (timer só existe enquanto falha).
fn ensure_icon(hwnd: HWND) {
    // SAFETY: janela viva desta thread.
    unsafe {
        if apply_icon(hwnd, NIM_ADD) {
            KillTimer(hwnd, RETRY_TIMER);
        } else {
            SetTimer(hwnd, RETRY_TIMER, RETRY_MS, None);
        }
    }
}

fn show_menu(hwnd: HWND) {
    let items: Vec<(usize, Vec<u16>, u32)> =
        menu_items(&status()).into_iter().map(|(id, text, flags)| (id, wide(&text), flags)).collect();
    // SAFETY: menu criado e destruído aqui; strings vivas durante a chamada.
    let command = unsafe {
        let menu = CreatePopupMenu();
        for (id, text, flags) in &items {
            AppendMenuW(menu, *flags, *id, if *flags & MF_SEPARATOR != 0 { ptr::null() } else { text.as_ptr() });
        }
        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        // Exigência do Windows para o menu fechar ao clicar fora.
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        command as usize
    };
    if command == MENU_ABOUT {
        // Thread própria: a caixa é modal e a janela da bandeja precisa seguir recebendo mensagens.
        let text = about_text(status().portuguese);
        let title = if status().portuguese { "Sobre o MouseX" } else { "About MouseX" };
        let _ =
            thread::Builder::new().name("mousex-about".into()).spawn(move || super::show_message(title, &text, false));
    } else if let Some(command) = command_for(command) {
        publish(Event::Tray(command));
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            if matches!((lparam & 0xFFFF) as u32, WM_LBUTTONUP | WM_RBUTTONUP) {
                show_menu(hwnd);
            }
            0
        }
        WM_STATUS => {
            apply_icon(hwnd, NIM_MODIFY);
            0
        }
        WM_WTSSESSION_CHANGE => {
            if wparam == WTS_SESSION_LOCK {
                publish(Event::SessionLocked);
            }
            0
        }
        WM_DISPLAYCHANGE => {
            publish(Event::DisplayChange);
            // SAFETY: repasse padrão.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_TIMER if wparam == RETRY_TIMER => {
            ensure_icon(hwnd);
            0
        }
        // Explorer reiniciado: o ícone sumiu e precisa ser recriado.
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            ensure_icon(hwnd);
            0
        }
        // SAFETY: repasse padrão.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Inicia o ícone numa thread própria. Os eventos só chegam a alguém depois de `Hooks::start`.
pub fn start_tray() -> io::Result<()> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::Builder::new().name("mousex-tray".into()).spawn(move || {
        let class = wide("MouseXTray");
        // SAFETY: classe e janela registradas e destruídas nesta thread; loop de mensagens padrão.
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(tray_proc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            if RegisterClassW(&wc) == 0 {
                let _ = ready_tx.send(Err(io::Error::last_os_error()));
                return;
            }
            // Janela de nível superior (não message-only): só elas recebem WM_DISPLAYCHANGE.
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if hwnd.is_null() {
                let _ = ready_tx.send(Err(io::Error::last_os_error()));
                return;
            }
            WINDOW.store(hwnd, Ordering::Release);
            let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            TASKBAR_CREATED.store(taskbar_created, Ordering::Relaxed);
            // Agente elevado: sem isto o UIPI descarta as mensagens do Explorer (nível médio).
            for message in [taskbar_created, WM_TRAY] {
                ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, ptr::null_mut());
            }
            // Bloqueio da sessão: o servidor bloqueia as outras máquinas junto.
            WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            ensure_icon(hwnd);
            let _ = ready_tx.send(Ok(()));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            WINDOW.store(ptr::null_mut(), Ordering::Release);
            Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
        }
    })?;
    ready_rx.recv().map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TrayStatus {
        TrayStatus {
            machines: vec![
                ("win2".into(), true, Some("1.4.0".into())),
                ("mac".into(), false, None),
                ("DESKTOP".into(), true, None),
            ],
            active: true,
            paused: false,
            locked: true,
            portuguese: true,
        }
    }

    #[test]
    fn menu_numera_maquinas_como_os_atalhos() {
        let items = menu_items(&sample());
        assert_eq!(items[0], (MENU_JUMP + 1, "●  win2\tCtrl+Alt+Shift+1    v1.4.0".into(), MF_STRING));
        assert_eq!(
            items[1],
            (MENU_JUMP + 2, "○  mac\tCtrl+Alt+Shift+2".into(), MF_STRING | MF_GRAYED),
            "desconectada fica cinza"
        );
        assert!(items.iter().any(|(id, _, flags)| *id == MENU_LOCK && flags & MF_CHECKED != 0), "cursor preso marcado");
        assert!(items.iter().any(|(id, _, flags)| *id == MENU_PAUSE && flags & MF_CHECKED == 0));
        let version = format!("MouseX {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(items.last(), Some(&(MENU_ABOUT, version, MF_STRING)), "versão é o último item");
    }

    #[test]
    fn comando_de_cada_item() {
        assert_eq!(command_for(MENU_JUMP + 3), Some(TrayCommand::Jump(3)));
        assert_eq!(command_for(MENU_RECONNECT), Some(TrayCommand::Reconnect));
        assert_eq!(command_for(0), None, "clicar fora do menu");
        assert_eq!(command_for(MENU_ABOUT), None, "o Sobre é aberto pela própria bandeja");
        assert_eq!(command_for(MENU_JUMP + 10), None);
    }

    #[test]
    fn dica_mostra_conectadas_e_estado() {
        assert_eq!(tooltip(&sample()), "MouseX (cursor preso) — win2, DESKTOP");
        assert_eq!(
            tooltip(&TrayStatus { portuguese: true, ..TrayStatus::default() }),
            "MouseX — nenhuma outra máquina conectada"
        );
        assert_eq!(tooltip(&TrayStatus::default()), "MouseX — no other computer connected");
        let english = TrayStatus { portuguese: false, ..sample() };
        assert!(about_text(false).contains("License: MIT or Apache-2.0"));
        assert!(menu_items(&english).iter().any(|(id, text, _)| *id == MENU_ARRANGE && text == "Arrange screens…"));
    }
}
