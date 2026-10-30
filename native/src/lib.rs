//! Backend Windows: hooks low-level com supressão sem bloqueio, injeção por scancode/posição absoluta e clipboard.
#![cfg(windows)]

use std::{
    collections::BTreeSet,
    io,
    mem::size_of,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU8, Ordering},
        mpsc, OnceLock,
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{
        DataExchange::*,
        LibraryLoader::GetModuleHandleW,
        Memory::*,
        SystemInformation::GetTickCount,
        Threading::{GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL},
    },
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

pub const INPUT_MARKER: usize = 0x4d58524d;
/// Texto: até 10 MiB em unidades UTF-16, terminador incluso.
pub const MAX_CLIPBOARD_UNITS: usize = 10 * 1024 * 1024;

mod access;
mod clip_files;
mod clip_image;
mod display;
mod env_path;
mod resilience;
pub use clip_image::{clip_from_png, dib_to_png, png_to_dib, Clip, MAX_CLIPBOARD_IMAGE_BYTES};
pub use env_path::set_in_system_path;

#[derive(Clone)]
pub struct Monitor {
    pub name: String,
    pub bounds: RECT,
    pub primary: bool,
}

/// Coordenadas em pixels físicos: chamar uma vez no início do processo, antes de criar janelas.
/// Retorna `false` se não foi possível (coordenadas do cursor e do hook podem divergir).
pub fn init_dpi_awareness() -> bool {
    // SAFETY: chamada sem ponteiros.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0 }
}

pub fn monitors() -> io::Result<Vec<Monitor>> {
    unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let out = &mut *(data as *mut (Vec<Monitor>, bool));
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(handle, &mut info.monitorInfo) == 0 {
            out.1 = true;
            return 0;
        }
        let end = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
        out.0.push(Monitor {
            name: String::from_utf16_lossy(&info.szDevice[..end]),
            bounds: info.monitorInfo.rcMonitor,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
        1
    }
    let mut out = (Vec::new(), false);
    if unsafe { EnumDisplayMonitors(ptr::null_mut(), ptr::null(), Some(collect), &mut out as *mut _ as isize) } == 0
        || out.1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(out.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// `Move` é posição absoluta no desktop virtual, tanto capturada quanto injetada.
/// `Delta` só é capturado: movimento bloqueado pela supressão, relativo ao cursor estacionado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key {
        scancode: u16,
        extended: bool,
        down: bool,
    },
    Button {
        button: Button,
        down: bool,
    },
    Move {
        x: i32,
        y: i32,
    },
    Delta {
        dx: i32,
        dy: i32,
    },
    Wheel {
        delta: i16,
        horizontal: bool,
    },
    /// Troca de desktop (bloqueio, Ctrl+Alt+Del, UAC): teclas seguradas nunca terão key-up visível.
    DesktopSwitch,
    /// Item escolhido no menu do ícone da bandeja.
    Tray(TrayCommand),
    /// Monitor plugado, desplugado ou resolução alterada.
    DisplayChange,
    /// Atalho reconhecido (e engolido) no próprio hook.
    Hotkey(Hotkey),
    /// Sessão bloqueada (Win+L, Ctrl+Alt+Del → Bloquear, inatividade).
    SessionLocked,
    /// O Windows tinha removido os hooks em silêncio e eles foram reinstalados (`false`: não deu).
    HooksRestored(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    /// Ctrl+Alt+Shift+1…9: N-ésima máquina da esquerda para a direita.
    Jump(u8),
    /// Scroll Lock: prende o cursor na tela atual.
    ToggleLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Arrange,
    ShowLog,
    Pause,
    LockCursor,
    Reconnect,
    /// Índice (1…) na ordem da esquerda para a direita, como os atalhos.
    Jump(u8),
}

mod identify;
mod panel;
mod power;
mod shell;
mod tray;
pub use access::{as_standard_user, create_private_dir, free_space};
pub use display::set_monitor_positions;
pub use identify::{show_identify, IdentifyScreen};
pub use panel::{show_panel, Cancel, Frame, Poll};
pub use power::{keep_awake, lock_workstation};
pub use resilience::KillOnCloseJob;
pub use shell::{hide_console, run_elevated, show_message};
pub use tray::{set_tray_status, start_tray, TrayStatus};

/// Estado dos modificadores no próprio hook: o atalho de emergência funciona mesmo se o resto do processo travar.
#[derive(Debug, Default, Clone, Copy)]
pub struct Modifiers {
    bits: u8,
}

impl Modifiers {
    /// Atualiza com a tecla; `true` quando é Ctrl+Alt+Shift+Home (Home dedicado, scancode estendido).
    pub fn key(&mut self, scancode: u16, extended: bool, down: bool) -> bool {
        let bit = match (scancode, extended) {
            (0x1D, false) => 1,
            (0x1D, true) => 2,
            (0x38, false) => 4,
            (0x38, true) => 8,
            (0x2A, _) => 16,
            (0x36, _) => 32,
            _ => 0,
        };
        if bit != 0 {
            if down {
                self.bits |= bit
            } else {
                self.bits &= !bit
            }
            return false;
        }
        down && (scancode, extended) == (0x47, true) && self.all_held()
    }

    fn all_held(&self) -> bool {
        self.bits & 0b11 != 0 && self.bits & 0b1100 != 0 && self.bits & 0b11_0000 != 0
    }

    /// Atalho que o hook engole (não chega ao app local nem ao remoto): Ctrl+Alt+Shift+1…9 e Scroll Lock.
    pub fn hotkey(&self, scancode: u16, extended: bool) -> Option<Hotkey> {
        match (scancode, extended) {
            (0x46, false) => Some(Hotkey::ToggleLock),
            (0x02..=0x0A, false) if self.all_held() => Some(Hotkey::Jump((scancode - 1) as u8)),
            _ => None,
        }
    }
}

/// Teclas de atalho engolidas: a soltura só é engolida se o aperto foi, e a repetição não dispara de novo.
#[derive(Debug, Default, Clone, Copy)]
pub struct HotkeyLatch {
    /// Bit por tecla de atalho apertada e engolida (1…9 e Scroll Lock).
    held: u16,
}

impl HotkeyLatch {
    fn bit(scancode: u16, extended: bool) -> Option<u16> {
        match (scancode, extended) {
            (0x02..=0x0A, false) => Some(1 << (scancode - 2)),
            (0x46, false) => Some(1 << 9),
            _ => None,
        }
    }

    /// (engolir, atalho a disparar).
    pub fn key(&mut self, modifiers: &Modifiers, scancode: u16, extended: bool, down: bool) -> (bool, Option<Hotkey>) {
        let Some(bit) = Self::bit(scancode, extended) else { return (false, None) };
        if !down {
            let swallowed = self.held & bit != 0;
            self.held &= !bit;
            return (swallowed, None);
        }
        if self.held & bit != 0 {
            return (true, None);
        }
        match modifiers.hotkey(scancode, extended) {
            Some(hotkey) => {
                self.held |= bit;
                (true, Some(hotkey))
            }
            None => (false, None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardChoice {
    Files,
    Text,
    Png,
    Dib,
    None,
}

/// Texto primeiro: planilhas publicam texto e imagem, e quem cola quer o texto. Arquivos por último: o Explorer
/// só publica arquivos, e o navegador publica a imagem copiada junto com um arquivo temporário dela.
pub fn clipboard_choice(has_files: bool, has_text: bool, has_png: bool, has_dib: bool) -> ClipboardChoice {
    if has_text {
        ClipboardChoice::Text
    } else if has_png {
        ClipboardChoice::Png
    } else if has_dib {
        ClipboardChoice::Dib
    } else if has_files {
        ClipboardChoice::Files
    } else {
        ClipboardChoice::None
    }
}

/// Com supressão, o cursor real só se move quando o próprio programa manda: ponto do hook menos a
/// posição atual do cursor é exatamente o movimento físico, sem depender de onde o cursor foi estacionado.
/// Movimentos gerados antes da supressão continuam `Move` e não viram deslocamento remoto.
pub fn hook_move(x: i32, y: i32, suppressed: bool, cursor: (i32, i32)) -> Event {
    hook_move_at(x, y, suppressed, cursor, 0, 0)
}

/// Como `hook_move`, mas descarta movimentos gerados antes do cursor ser estacionado (`time` < `park_tick`):
/// o ponto deles é relativo à posição antiga e viraria um salto falso de meia tela no remoto.
pub fn hook_move_at(x: i32, y: i32, suppressed: bool, cursor: (i32, i32), time: u32, park_tick: u32) -> Event {
    if suppressed && (time.wrapping_sub(park_tick) as i32) >= 0 {
        Event::Delta { dx: x - cursor.0, dy: y - cursor.1 }
    } else {
        Event::Move { x, y }
    }
}

/// Cria a pasta já com a ACL final (Administradores e SYSTEM: total; Usuários: leitura), sem herdar nada de
/// ProgramData. Falha se ela já existe: pasta criada por outro programa antes (ou durante) nunca é adotada.
pub fn create_protected_dir(path: &std::path::Path) -> io::Result<()> {
    create_dir_with_sddl(path, "D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;BU)")
}

/// Cria a pasta com o descritor `sddl` desde o primeiro instante; falha se ela já existe.
pub(crate) fn create_dir_with_sddl(path: &std::path::Path, sddl: &str) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Security::{
            Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::CreateDirectoryW,
    };
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: strings terminadas em NUL; descritor alocado pelo sistema e liberado com LocalFree.
    unsafe {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let created = CreateDirectoryW(wide.as_ptr(), &attributes) != 0;
        let error = io::Error::last_os_error();
        LocalFree(descriptor as _);
        if created {
            Ok(())
        } else {
            Err(error)
        }
    }
}

/// Abre um arquivo numa pasta que um usuário comum controla sem seguir links: recusa ponto de reanálise e
/// confere o caminho final do próprio handle (junction numa pasta acima ou link do gerenciador de objetos
/// mudariam o caminho). Sem corrida entre checar e usar: tudo vale para o handle aberto.
pub fn open_unlinked(path: &std::path::Path, delete: bool) -> io::Result<std::fs::File> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ,
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if delete {
        options.access_mode(FILE_GENERIC_READ | DELETE);
    }
    let file = options.open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(format!("{} is a link", path.display())));
    }
    let mut buffer = vec![0u16; 1024];
    // SAFETY: handle vivo; buffer do tamanho informado.
    let length =
        unsafe { GetFinalPathNameByHandleW(file.as_raw_handle() as _, buffer.as_mut_ptr(), buffer.len() as u32, 0) }
            as usize;
    if length == 0 || length >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    let real = String::from_utf16_lossy(&buffer[..length]);
    let real = real
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| real.trim_start_matches(r"\\?\").to_string());
    if !real.eq_ignore_ascii_case(&long_path(path)) {
        return Err(io::Error::other(format!("{} resolves to {real}", path.display())));
    }
    Ok(file)
}

/// Caminho com os nomes curtos do DOS ("RUNNER~1") expandidos. Não segue junctions: cada componente
/// mantém o próprio nome, então o desvio por link continua aparecendo na comparação com o caminho final.
fn long_path(path: &std::path::Path) -> String {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: caminho terminado em NUL; buffer do tamanho informado.
    let length = unsafe { GetLongPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return path.display().to_string();
    }
    std::ffi::OsString::from_wide(&buffer[..length]).to_string_lossy().into_owned()
}

/// Apaga pelo handle de `open_unlinked`: o arquivo apagado é exatamente o que foi conferido.
pub fn remove_unlinked(path: &std::path::Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let file = open_unlinked(path, true)?;
    let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: handle vivo com acesso DELETE; estrutura do tamanho informado.
    let done = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as _,
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if done == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Dono da pasta é Administradores ou SYSTEM (e não um usuário comum que a criou antes da instalação).
pub fn owned_by_admins(path: &std::path::Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{
        Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT},
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: caminho terminado em NUL; descritor e string alocados pelo sistema e liberados com LocalFree.
    unsafe {
        let mut owner: PSID = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut text: windows_sys::core::PWSTR = ptr::null_mut();
        let converted = ConvertSidToStringSidW(owner, &mut text) != 0;
        let sid = if converted {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text as _);
            sid
        } else {
            String::new()
        };
        LocalFree(descriptor as _);
        if !converted {
            return Err(io::Error::last_os_error());
        }
        Ok(sid == "S-1-5-32-544" || sid == "S-1-5-18")
    }
}

fn current_cursor() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: ponteiro para POINT local válido.
    unsafe { GetCursorPos(&mut point) };
    (point.x, point.y)
}

pub fn is_own_input(extra: usize) -> bool {
    extra == INPUT_MARKER
}
pub fn mouse_button(message: u32, data: u32) -> Option<(Button, bool)> {
    Some(match message {
        WM_LBUTTONDOWN => (Button::Left, true),
        WM_LBUTTONUP => (Button::Left, false),
        WM_RBUTTONDOWN => (Button::Right, true),
        WM_RBUTTONUP => (Button::Right, false),
        WM_MBUTTONDOWN => (Button::Middle, true),
        WM_MBUTTONUP => (Button::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => (
            match data >> 16 {
                1 => Button::X1,
                2 => Button::X2,
                _ => return None,
            },
            message == WM_XBUTTONDOWN,
        ),
        _ => return None,
    })
}

#[derive(Default, Debug)]
pub struct InputState {
    keys: BTreeSet<(u16, bool)>,
    buttons: BTreeSet<Button>,
}
impl InputState {
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Key { scancode, extended, down } => {
                if down {
                    self.keys.insert((scancode, extended));
                } else {
                    self.keys.remove(&(scancode, extended));
                }
            }
            Event::Button { button, down } => {
                if down {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
            }
            _ => {}
        }
    }
    pub fn releases(&self) -> Vec<Event> {
        self.keys
            .iter()
            .map(|&(scancode, extended)| Event::Key { scancode, extended, down: false })
            .chain(self.buttons.iter().map(|&button| Event::Button { button, down: false }))
            .collect()
    }
}

/// Converte pixel do desktop virtual para a escala 0..=65535 do `MOUSEEVENTF_VIRTUALDESK`, clampando.
pub fn to_absolute(x: i32, y: i32, vx: i32, vy: i32, vw: i32, vh: i32) -> (i32, i32) {
    let axis = |value: i32, origin: i32, size: i32| -> i32 {
        let span = (size - 1).max(1) as i64;
        let offset = (value as i64 - origin as i64).clamp(0, span);
        (offset * 65535 / span) as i32
    };
    (axis(x, vx, vw), axis(y, vy, vh))
}

type Sender = fn(&INPUT) -> io::Result<()>;

fn send_input(input: &INPUT) -> io::Result<()> {
    // SAFETY: um INPUT válido, tamanho correto.
    if unsafe { SendInput(1, input, size_of::<INPUT>() as i32) } != 1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Registra apenas o que foi injetado com sucesso; teclas físicas locais não entram.
pub struct Injector {
    state: InputState,
    sender: Sender,
}
impl Default for Injector {
    fn default() -> Self {
        Self::with_sender(send_input)
    }
}
impl Injector {
    pub fn with_sender(sender: Sender) -> Self {
        Self { state: InputState::default(), sender }
    }

    pub fn send(&mut self, event: Event) -> io::Result<()> {
        // SAFETY: INPUT é POD; preenchemos apenas a variante da união indicada em `r#type`.
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        match event {
            Event::Delta { .. }
            | Event::DesktopSwitch
            | Event::Tray(_)
            | Event::DisplayChange
            | Event::Hotkey(_)
            | Event::HooksRestored(_)
            | Event::SessionLocked => return Err(io::Error::new(io::ErrorKind::InvalidInput, "evento só de captura")),
            Event::Key { scancode, extended, down } => {
                // O hook marca NumLock como 0x45 estendido e Pause como 0x45 simples, mas na injeção E0 45 não
                // vira tecla nenhuma e 0x45 simples vira NumLock. Pause (E1 1D 45) só sai por virtual key.
                let (vk, scan, flags) = match (scancode, extended) {
                    (0x45, true) => (0, 0x45, KEYEVENTF_SCANCODE),
                    (0x45, false) => (VK_PAUSE, 0x45, 0),
                    (_, true) => (0, scancode, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY),
                    (_, false) => (0, scancode, KEYEVENTF_SCANCODE),
                };
                input.r#type = INPUT_KEYBOARD;
                input.Anonymous.ki = KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags | if down { 0 } else { KEYEVENTF_KEYUP },
                    time: 0,
                    dwExtraInfo: INPUT_MARKER,
                };
            }
            _ => {
                input.r#type = INPUT_MOUSE;
                let (dx, dy, data, flags) = match event {
                    Event::Move { x, y } => {
                        // SAFETY: GetSystemMetrics sem ponteiros.
                        let (vx, vy, vw, vh) = unsafe {
                            (
                                GetSystemMetrics(SM_XVIRTUALSCREEN),
                                GetSystemMetrics(SM_YVIRTUALSCREEN),
                                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                            )
                        };
                        let (ax, ay) = to_absolute(x, y, vx, vy, vw, vh);
                        (ax, ay, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)
                    }
                    Event::Wheel { delta, horizontal } => {
                        (0, 0, delta as i32 as u32, if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL })
                    }
                    Event::Button { button, down } => {
                        let (data, flags) = button_flags(button, down);
                        (0, 0, data, flags)
                    }
                    Event::Key { .. }
                    | Event::Delta { .. }
                    | Event::DesktopSwitch
                    | Event::Tray(_)
                    | Event::DisplayChange
                    | Event::Hotkey(_)
                    | Event::HooksRestored(_)
                    | Event::SessionLocked => unreachable!("tratado acima"),
                };
                input.Anonymous.mi =
                    MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: INPUT_MARKER };
            }
        }
        (self.sender)(&input)?;
        self.state.apply(event);
        Ok(())
    }

    /// Tenta todas as liberações; falhas ficam retidas para nova tentativa.
    pub fn release_all(&mut self) -> io::Result<()> {
        let mut first = None;
        for event in self.state.releases() {
            if let Err(e) = self.send(event) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}
impl Drop for Injector {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}
fn button_flags(button: Button, down: bool) -> (u32, u32) {
    match (button, down) {
        (Button::Left, true) => (0, MOUSEEVENTF_LEFTDOWN),
        (Button::Left, false) => (0, MOUSEEVENTF_LEFTUP),
        (Button::Right, true) => (0, MOUSEEVENTF_RIGHTDOWN),
        (Button::Right, false) => (0, MOUSEEVENTF_RIGHTUP),
        (Button::Middle, true) => (0, MOUSEEVENTF_MIDDLEDOWN),
        (Button::Middle, false) => (0, MOUSEEVENTF_MIDDLEUP),
        (b, d) => (if b == Button::X1 { 1 } else { 2 }, if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }),
    }
}

/// Estaciona o cursor no centro do monitor principal (longe das bordas, onde o Windows corta o movimento)
/// e devolve a posição REAL resultante: `SetCursorPos` pode falhar (UIPI, desktop seguro).
pub fn park_cursor() -> (i32, i32) {
    // SAFETY: chamadas sem ponteiros.
    let target = unsafe { (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2) };
    // Algumas tentativas: na borda, um SetCursorPos recusado deixaria o remoto sem andar para aquele lado.
    for _ in 0..3 {
        // SAFETY: chamada sem ponteiros.
        unsafe { SetCursorPos(target.0, target.1) };
        if current_cursor() == target {
            break;
        }
    }
    // SAFETY: chamada sem ponteiros.
    PARK_TICK.store(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() }, Ordering::Relaxed);
    current_cursor()
}

pub fn set_cursor(x: i32, y: i32) {
    // SAFETY: chamada sem ponteiros.
    unsafe { SetCursorPos(x, y) };
}

// Caminho quente do hook: só atômico + envio sem bloqueio (canal ilimitado). Nada de lock, rede ou log.
static SENDER: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
static SUPPRESS: AtomicBool = AtomicBool::new(false);
static MODIFIERS: AtomicU8 = AtomicU8::new(0);
/// Só a thread do hook escreve: teclas de atalho engolidas (ver `HotkeyLatch`).
static HOTKEY_LATCH: AtomicU16 = AtomicU16::new(0);
/// Instante (GetTickCount) em que o cursor foi estacionado pela última vez.
static PARK_TICK: AtomicU32 = AtomicU32::new(0);
/// Hora (ms do sistema) do último evento que o hook de mouse viu: prova de que ele ainda está instalado.
static LAST_HOOK_TICK: AtomicU32 = AtomicU32::new(0);

fn publish(event: Event) -> bool {
    if let Some(sender) = SENDER.get() {
        let _ = sender.send(event);
    }
    SUPPRESS.load(Ordering::Relaxed)
}

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: com HC_ACTION, `data` aponta para KBDLLHOOKSTRUCT válido durante o callback.
        let key = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        if !is_own_input(key.dwExtraInfo)
            && matches!(message as u32, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP)
        {
            let (scancode, extended) = (key.scanCode as u16, key.flags & LLKHF_EXTENDED != 0);
            let down = matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            // Só a thread do hook escreve MODIFIERS: load/store simples bastam.
            let mut modifiers = Modifiers { bits: MODIFIERS.load(Ordering::Relaxed) };
            if modifiers.key(scancode, extended, down) {
                SUPPRESS.store(false, Ordering::Relaxed);
            }
            MODIFIERS.store(modifiers.bits, Ordering::Relaxed);
            let mut latch = HotkeyLatch { held: HOTKEY_LATCH.load(Ordering::Relaxed) };
            let (swallow, fired) = latch.key(&modifiers, scancode, extended, down);
            HOTKEY_LATCH.store(latch.held, Ordering::Relaxed);
            if let Some(hotkey) = fired {
                publish(Event::Hotkey(hotkey));
            }
            if swallow {
                return 1;
            }
            let suppress = publish(Event::Key { scancode, extended, down });
            if suppress {
                return 1;
            }
        }
    }
    // SAFETY: repasse padrão da cadeia de hooks.
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

unsafe extern "system" fn mouse(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: com HC_ACTION, `data` aponta para MSLLHOOKSTRUCT válido durante o callback.
        let m = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
        LAST_HOOK_TICK.store(m.time, Ordering::Relaxed);
        if !is_own_input(m.dwExtraInfo) {
            let event = if let Some((button, down)) = mouse_button(message as u32, m.mouseData) {
                Some(Event::Button { button, down })
            } else {
                match message as u32 {
                    WM_MOUSEMOVE => {
                        let suppressed = SUPPRESS.load(Ordering::Relaxed);
                        // GetCursorPos só quando suprimido: uma chamada barata, sem lock nem espera.
                        let cursor = if suppressed { current_cursor() } else { (0, 0) };
                        Some(hook_move_at(
                            m.pt.x,
                            m.pt.y,
                            suppressed,
                            cursor,
                            m.time,
                            PARK_TICK.load(Ordering::Relaxed),
                        ))
                    }
                    WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some(Event::Wheel {
                        delta: (m.mouseData >> 16) as i16,
                        horizontal: message as u32 == WM_MOUSEHWHEEL,
                    }),
                    _ => None,
                }
            };
            if let Some(event) = event {
                if publish(event) {
                    return 1;
                }
            }
        }
    }
    // SAFETY: repasse padrão da cadeia de hooks.
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

unsafe extern "system" fn desktop_switch(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    // Teclas seguradas ficaram no outro desktop: zera o estado local e devolve a entrada à casa.
    MODIFIERS.store(0, Ordering::Relaxed);
    SUPPRESS.store(false, Ordering::Relaxed);
    publish(Event::DesktopSwitch);
}

/// Hooks de teclado e mouse numa thread própria de prioridade crítica. Um por processo.
pub struct Hooks {
    thread_id: u32,
    thread: Option<thread::JoinHandle<io::Result<()>>>,
}
impl Hooks {
    pub fn start() -> io::Result<(Self, mpsc::Receiver<Event>)> {
        let (tx, rx) = mpsc::channel();
        SENDER
            .set(tx)
            .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "hooks já iniciados neste processo"))?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("mousex-hooks".into()).spawn(move || {
            // SAFETY: API Win32 na própria thread; hooks removidos antes de sair.
            unsafe {
                SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
                let mut msg: MSG = std::mem::zeroed();
                PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE);
                let module = GetModuleHandleW(ptr::null());
                let mut kh = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), module, 0);
                if kh.is_null() {
                    let _ = ready_tx.send(Err(io::Error::last_os_error()));
                    return Ok(());
                }
                let mut mh = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), module, 0);
                if mh.is_null() {
                    let error = io::Error::last_os_error();
                    UnhookWindowsHookEx(kh);
                    let _ = ready_tx.send(Err(error));
                    return Ok(());
                }
                let switch_hook = SetWinEventHook(
                    EVENT_SYSTEM_DESKTOPSWITCH,
                    EVENT_SYSTEM_DESKTOPSWITCH,
                    ptr::null_mut(),
                    Some(desktop_switch),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                );
                let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                // Timer da própria thread (WM_TIMER sem janela) para a checagem de vida dos hooks.
                let timer = SetTimer(ptr::null_mut(), 0, resilience::HOOK_CHECK_MS, None);
                let mut liveness = resilience::Liveness::default();
                let result = loop {
                    match GetMessageW(&mut msg, ptr::null_mut(), 0, 0) {
                        -1 => break Err(io::Error::last_os_error()),
                        0 => break Ok(()),
                        _ if msg.message == WM_TIMER && msg.hwnd.is_null() => {
                            let last = LAST_HOOK_TICK.load(Ordering::Relaxed);
                            if liveness.observe(current_cursor(), GetTickCount(), last) {
                                UnhookWindowsHookEx(mh);
                                UnhookWindowsHookEx(kh);
                                kh = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), module, 0);
                                mh = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), module, 0);
                                publish(Event::HooksRestored(!kh.is_null() && !mh.is_null()));
                            }
                        }
                        _ => {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                };
                SUPPRESS.store(false, Ordering::Relaxed);
                if timer != 0 {
                    KillTimer(ptr::null_mut(), timer);
                }
                if !switch_hook.is_null() {
                    UnhookWinEvent(switch_hook);
                }
                UnhookWindowsHookEx(mh);
                UnhookWindowsHookEx(kh);
                result
            }
        })?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((Self { thread_id, thread: Some(thread) }, rx)),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                let _ = thread.join();
                Err(io::Error::other(error))
            }
        }
    }

    /// Com `true`, entrada física local deixa de chegar às aplicações (continua publicada).
    pub fn set_suppress(on: bool) {
        SUPPRESS.store(on, Ordering::Relaxed);
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        let Some(thread) = self.thread.take() else { return Ok(()) };
        SUPPRESS.store(false, Ordering::Relaxed);
        // SAFETY: id de thread válido obtido na criação.
        if unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) } == 0 && !thread.is_finished() {
            return Err(io::Error::last_os_error());
        }
        thread.join().map_err(|_| io::Error::other("input hook thread panicked"))?
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct Clipboard(HWND);
impl Clipboard {
    fn open() -> io::Result<Self> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        unsafe {
            let window = CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            );
            if window.is_null() {
                return Err(io::Error::last_os_error());
            }
            if OpenClipboard(window) == 0 {
                let error = io::Error::last_os_error();
                DestroyWindow(window);
                return Err(error);
            }
            Ok(Self(window))
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
            DestroyWindow(self.0);
        }
    }
}
pub fn clipboard_text() -> io::Result<Option<String>> {
    let _guard = Clipboard::open()?;
    unsafe {
        if IsClipboardFormatAvailable(13) == 0 {
            return Ok(None);
        }
        let handle = GetClipboardData(13);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let units = GlobalSize(handle) / 2;
        if units == 0 || units > MAX_CLIPBOARD_UNITS {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "clipboard size limit"));
        }
        let data = GlobalLock(handle) as *const u16;
        if data.is_null() {
            return Err(io::Error::last_os_error());
        }
        let slice = std::slice::from_raw_parts(data, units);
        let result = slice
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Unicode terminator"))
            .and_then(|n| String::from_utf16(&slice[..n]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)));
        GlobalUnlock(handle);
        result.map(Some)
    }
}
pub fn set_clipboard_text(text: &str) -> io::Result<()> {
    if text.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "embedded NUL"));
    }
    let units = text.encode_utf16().count() + 1;
    if units > MAX_CLIPBOARD_UNITS {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "clipboard size limit"));
    }
    let _guard = Clipboard::open()?;
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, units * 2);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let data = GlobalLock(handle) as *mut u16;
        if data.is_null() {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        for (i, unit) in text.encode_utf16().chain(Some(0)).enumerate() {
            data.add(i).write(unit);
        }
        GlobalUnlock(handle);
        if EmptyClipboard() == 0 || SetClipboardData(13, handle).is_null() {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        // Win32 owns handle after successful SetClipboardData.
        Ok(())
    }
}

const CF_DIB_FORMAT: u32 = 8;
/// DIB bruto de print 4K com folga; PNG resultante ainda passa pelo limite de 32 MiB.
const MAX_CLIPBOARD_RAW_BYTES: usize = 128 * 1024 * 1024;

/// Muda a cada alteração do clipboard; leitura barata, sem abrir o clipboard.
pub fn clipboard_seq() -> u32 {
    // SAFETY: chamada sem ponteiros.
    unsafe { GetClipboardSequenceNumber() }
}

const CF_HDROP_FORMAT: u32 = 15;
const DROPEFFECT_COPY: u32 = 1;

fn png_format() -> u32 {
    register_format("PNG")
}

fn register_format(name: &str) -> u32 {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: string UTF-16 terminada em NUL.
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

/// Copia o bloco global de `format`; o clipboard precisa estar aberto.
fn read_global(format: u32) -> io::Result<Vec<u8>> {
    // SAFETY: clipboard aberto pelo chamador; Lock/Unlock balanceados e cópia limitada a GlobalSize.
    unsafe {
        let handle = GetClipboardData(format);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let size = GlobalSize(handle);
        if size == 0 || size > MAX_CLIPBOARD_RAW_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "tamanho de clipboard fora do limite"));
        }
        let data = GlobalLock(handle) as *const u8;
        if data.is_null() {
            return Err(io::Error::last_os_error());
        }
        let bytes = std::slice::from_raw_parts(data, size).to_vec();
        GlobalUnlock(handle);
        Ok(bytes)
    }
}

/// Grava `bytes` em `format`; o clipboard precisa estar aberto e esvaziado.
fn set_global(format: u32, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: bloco alocado com o tamanho exato; posse passa ao Windows só após SetClipboardData bem-sucedido.
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let data = GlobalLock(handle) as *mut u8;
        if data.is_null() {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        GlobalUnlock(handle);
        if SetClipboardData(format, handle).is_null() {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        Ok(())
    }
}

/// Prefere PNG registrado, depois CF_DIB (convertido para PNG), depois texto Unicode.
pub fn clipboard_read() -> io::Result<Option<Clip>> {
    {
        let _guard = Clipboard::open()?;
        let png = png_format();
        // SAFETY: consultas de formato com o clipboard aberto.
        let available = |format: u32| format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0;
        match clipboard_choice(available(CF_HDROP_FORMAT), available(13), available(png), available(CF_DIB_FORMAT)) {
            ClipboardChoice::Files => {
                return Ok(clip_files::parse_dropfiles(&read_global(CF_HDROP_FORMAT)?)
                    .filter(|p| clip_files::usable(p))
                    .map(Clip::Files));
            }
            ClipboardChoice::Png => return Ok(clip_from_png(read_global(png)?)),
            ClipboardChoice::Dib => return Ok(dib_to_png(&read_global(CF_DIB_FORMAT)?).and_then(clip_from_png)),
            ClipboardChoice::None => return Ok(None),
            ClipboardChoice::Text => {}
        }
    }
    Ok(clipboard_text()?.map(Clip::Text))
}

/// Imagem é gravada como PNG e CF_DIB, para apps que só entendem um dos dois.
/// Arquivos vão como CF_HDROP com efeito "copiar": colar nunca move (apaga) a origem.
pub fn clipboard_write(clip: &Clip) -> io::Result<()> {
    match clip {
        Clip::Files(paths) => {
            let _guard = Clipboard::open()?;
            // SAFETY: clipboard aberto por este processo.
            if unsafe { EmptyClipboard() } == 0 {
                return Err(io::Error::last_os_error());
            }
            set_global(CF_HDROP_FORMAT, &clip_files::build_dropfiles(paths))?;
            let effect = register_format("Preferred DropEffect");
            if effect != 0 {
                set_global(effect, &DROPEFFECT_COPY.to_le_bytes())?;
            }
            Ok(())
        }
        Clip::Text(text) => set_clipboard_text(text),
        Clip::Png(png) => {
            let dib = png_to_dib(png).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PNG inválido"))?;
            let _guard = Clipboard::open()?;
            // SAFETY: clipboard aberto por este processo.
            if unsafe { EmptyClipboard() } == 0 {
                return Err(io::Error::last_os_error());
            }
            set_global(CF_DIB_FORMAT, &dib)?;
            let format = png_format();
            if format != 0 {
                set_global(format, png)?;
            }
            Ok(())
        }
    }
}

/// Diálogo Sim/Não modal, em primeiro plano. Bloqueia: chamar fora do loop de eventos.
pub fn ask_yes_no(title: &str, text: &str) -> bool {
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (title, text) = (wide(title), wide(text));
    // SAFETY: strings UTF-16 terminadas em NUL, vivas durante a chamada.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND | MB_SYSTEMMODAL,
        ) == IDYES
    }
}

/// Pasta Program Files pelo shell do Windows, nunca pela variável de ambiente (que o usuário pode trocar).
pub fn program_files_dir() -> io::Result<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_ProgramFiles)
}

/// ProgramData pelo shell (mesmo motivo): dados do agente elevado ficam onde só administradores escrevem.
pub fn program_data_dir() -> io::Result<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_ProgramData)
}

/// Processo com token elevado (administrador efetivo).
pub fn is_elevated() -> bool {
    use windows_sys::Win32::{
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    // SAFETY: handle de token próprio, buffer TOKEN_ELEVATION local com tamanho correto; handle fechado.
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        ) != 0;
        CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn known_folder(id: &windows_sys::core::GUID) -> io::Result<std::path::PathBuf> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath};
    let mut raw: windows_sys::core::PWSTR = ptr::null_mut();
    // SAFETY: GUID válido; `raw` recebe string alocada pelo shell, liberada com CoTaskMemFree.
    unsafe {
        let status = SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut raw);
        if status != 0 || raw.is_null() {
            if !raw.is_null() {
                CoTaskMemFree(raw as *const _);
            }
            return Err(io::Error::other(format!("SHGetKnownFolderPath failed: {status:#x}")));
        }
        let length = (0..).take_while(|&i| *raw.add(i) != 0).count();
        let path = String::from_utf16_lossy(std::slice::from_raw_parts(raw, length));
        CoTaskMemFree(raw as *const _);
        Ok(path.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_releases_only_held_inputs() {
        let mut state = InputState::default();
        state.apply(Event::Key { scancode: 0x1E, extended: false, down: true });
        state.apply(Event::Key { scancode: 0x1E, extended: false, down: true });
        state.apply(Event::Button { button: Button::X2, down: true });
        state.apply(Event::Key { scancode: 0x1E, extended: false, down: false });
        assert_eq!(state.releases(), vec![Event::Button { button: Button::X2, down: false }]);
        for event in state.releases() {
            state.apply(event);
        }
        assert!(state.releases().is_empty());
    }
    #[test]
    fn maps_buttons_and_signed_wheel() {
        assert_eq!(mouse_button(WM_XBUTTONUP, 2 << 16), Some((Button::X2, false)));
        assert_eq!(mouse_button(WM_XBUTTONDOWN, 3 << 16), None);
        assert_eq!(button_flags(Button::Right, false), (0, MOUSEEVENTF_RIGHTUP));
        assert_eq!((0xff88_0000u32 >> 16) as i16, -120);
    }
    #[test]
    fn filters_only_own_marker() {
        assert!(is_own_input(INPUT_MARKER));
        assert!(!is_own_input(0));
        assert!(!is_own_input(INPUT_MARKER + 1));
    }

    #[test]
    fn normaliza_coordenada_absoluta() {
        assert_eq!(to_absolute(0, 0, 0, 0, 1920, 1080), (0, 0));
        assert_eq!(to_absolute(1919, 1079, 0, 0, 1920, 1080), (65535, 65535));
        assert_eq!(to_absolute(-1920, 0, -1920, 0, 3840, 1080), (0, 0));
        assert_eq!(to_absolute(1919, 0, -1920, 0, 3840, 1080), (65535, 0));
        assert_eq!(to_absolute(5000, -50, 0, 0, 1920, 1080), (65535, 0));
    }

    thread_local! { static SENT: std::cell::RefCell<Vec<(u32, u16, u32)>> = const { std::cell::RefCell::new(Vec::new()) }; }

    fn record(input: &INPUT) -> io::Result<()> {
        // SAFETY: o injetor só monta INPUT_KEYBOARD/INPUT_MOUSE com a união correspondente.
        let entry = unsafe {
            if input.r#type == INPUT_KEYBOARD {
                (input.r#type, input.Anonymous.ki.wScan, input.Anonymous.ki.dwFlags)
            } else {
                (input.r#type, 0, input.Anonymous.mi.dwFlags)
            }
        };
        SENT.with(|sent| sent.borrow_mut().push(entry));
        Ok(())
    }

    #[test]
    fn numlock_sai_sem_flag_estendida_e_pause_por_virtual_key() {
        let mut injector = Injector::with_sender(record);
        SENT.with(|sent| sent.borrow_mut().clear());
        injector.send(Event::Key { scancode: 0x45, extended: true, down: true }).expect("numlock");
        injector.send(Event::Key { scancode: 0x45, extended: false, down: false }).expect("pause");
        let sent = SENT.with(|sent| sent.borrow().clone());
        assert_eq!(sent, [(INPUT_KEYBOARD, 0x45, KEYEVENTF_SCANCODE), (INPUT_KEYBOARD, 0x45, KEYEVENTF_KEYUP)]);
    }

    #[test]
    fn release_all_solta_scancodes_injetados() {
        let mut injector = Injector::with_sender(record);
        injector.send(Event::Key { scancode: 0x1D, extended: true, down: true }).expect("ctrl direito");
        injector.send(Event::Button { button: Button::Left, down: true }).expect("botão");
        SENT.with(|sent| sent.borrow_mut().clear());
        injector.release_all().expect("liberação");
        let sent = SENT.with(|sent| sent.borrow().clone());
        assert!(sent.contains(&(INPUT_KEYBOARD, 0x1D, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP)));
        assert!(sent.iter().any(|&(kind, _, flags)| kind == INPUT_MOUSE && flags == MOUSEEVENTF_LEFTUP));
        injector.release_all().expect("idempotente");
    }

    #[test]
    fn supressao_liga_e_desliga() {
        Hooks::set_suppress(true);
        assert!(SUPPRESS.load(Ordering::Relaxed));
        Hooks::set_suppress(false);
        assert!(!SUPPRESS.load(Ordering::Relaxed));
    }

    /// DIB 32bpp BI_RGB bottom-up: primeira linha na memória é a de baixo. Pixels BGRA.
    fn dib(width: i32, height: i32, rows_bottom_up: &[[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        for value in [40u32.to_le_bytes(), width.to_le_bytes(), height.to_le_bytes()] {
            out.extend(value);
        }
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend([0u8; 24]);
        for pixel in rows_bottom_up {
            out.extend(pixel);
        }
        out
    }

    #[test]
    fn dib_para_png_e_volta() {
        let original = dib(2, 2, &[[1, 2, 3, 255], [4, 5, 6, 255], [7, 8, 9, 255], [10, 11, 12, 255]]);
        let png = dib_to_png(&original).expect("png");
        assert_eq!(png_to_dib(&png).expect("dib"), original);
    }

    #[test]
    fn dib_bottom_up_vira_png_certo() {
        // Linha de baixo vermelha, de cima azul (BGRA).
        let png = dib_to_png(&dib(1, 2, &[[0, 0, 255, 255], [255, 0, 0, 255]])).expect("png");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("info");
        let mut rgba = vec![0u8; reader.output_buffer_size()];
        reader.next_frame(&mut rgba).expect("frame");
        assert_eq!(&rgba[..4], &[0, 0, 255, 255], "topo azul");
        assert_eq!(&rgba[4..8], &[255, 0, 0, 255], "base vermelha");
    }

    #[test]
    fn alfa_zerado_vira_opaco() {
        let png = dib_to_png(&dib(1, 1, &[[9, 9, 9, 0]])).expect("png");
        assert_eq!(png_to_dib(&png).expect("dib")[40..], [9, 9, 9, 255]);
    }

    #[test]
    fn imagem_acima_do_limite_e_ignorada() {
        assert!(clip_from_png(vec![0; MAX_CLIPBOARD_IMAGE_BYTES + 1]).is_none());
        assert!(matches!(clip_from_png(vec![1, 2, 3]), Some(Clip::Png(_))));
        assert!(dib_to_png(&[0u8; 10]).is_none());
    }

    #[test]
    fn program_files_ignora_variavel_de_ambiente() {
        std::env::set_var("ProgramFiles", r"C:\Users\Public\malicioso");
        let dir = program_files_dir().expect("pasta conhecida");
        assert!(!dir.display().to_string().contains("malicioso"));
        assert!(dir.ends_with("Program Files"));
    }

    #[test]
    fn hook_so_gera_delta_quando_suprimido() {
        assert_eq!(hook_move(10, 20, false, (5, 5)), Event::Move { x: 10, y: 20 });
        assert_eq!(hook_move(1725, 718, true, (1720, 720)), Event::Delta { dx: 5, dy: -2 });
        assert_eq!(hook_move(1720, 720, true, (1720, 720)), Event::Delta { dx: 0, dy: 0 });
    }

    #[test]
    fn delta_nao_e_injetavel() {
        let mut injector = Injector::with_sender(record);
        assert!(injector.send(Event::Delta { dx: 1, dy: 1 }).is_err());
    }

    #[test]
    fn atalho_de_emergencia_no_hook() {
        let mut mods = Modifiers::default();
        for (scancode, extended) in [(0x1D, false), (0x38, false), (0x2A, false)] {
            assert!(!mods.key(scancode, extended, true));
        }
        assert!(mods.key(0x47, true, true), "Ctrl+Alt+Shift+Home");
        assert!(!mods.key(0x47, false, true), "Home do teclado numérico não conta");
        mods.key(0x2A, false, false);
        assert!(!mods.key(0x47, true, true), "sem Shift não dispara");
        mods.key(0x36, false, true);
        assert!(mods.key(0x47, true, true), "Shift direito também vale");
    }

    #[test]
    fn atalho_so_engole_a_soltura_que_engoliu_e_ignora_repeticao() {
        let (mut mods, mut latch) = (Modifiers::default(), HotkeyLatch::default());
        assert_eq!(latch.key(&mods, 0x02, false, true), (false, None), "1 apertado antes dos modificadores passa");
        for (scancode, extended) in [(0x1D, false), (0x38, false), (0x2A, false)] {
            mods.key(scancode, extended, true);
        }
        assert_eq!(latch.key(&mods, 0x02, false, false), (false, None), "soltura do 1 que passou também passa");
        assert_eq!(latch.key(&mods, 0x03, false, true), (true, Some(Hotkey::Jump(2))));
        assert_eq!(latch.key(&mods, 0x03, false, true), (true, None), "repetição automática não pula de novo");
        mods.key(0x38, false, false);
        assert_eq!(latch.key(&mods, 0x03, false, false), (true, None), "soltura engolida mesmo sem Alt");
        assert_eq!(latch.key(&mods, 0x03, false, false), (false, None));
    }

    #[test]
    fn atalhos_de_maquina_e_scroll_lock_no_hook() {
        let mut mods = Modifiers::default();
        assert_eq!(mods.hotkey(0x46, false), Some(Hotkey::ToggleLock), "Scroll Lock sozinho prende o cursor");
        assert_eq!(mods.hotkey(0x46, true), None, "Ctrl+Break não é Scroll Lock");
        assert_eq!(mods.hotkey(0x02, false), None, "1 sem modificadores é só 1");
        for (scancode, extended) in [(0x1D, false), (0x38, false), (0x2A, false)] {
            mods.key(scancode, extended, true);
        }
        assert_eq!(mods.hotkey(0x02, false), Some(Hotkey::Jump(1)));
        assert_eq!(mods.hotkey(0x0A, false), Some(Hotkey::Jump(9)));
        assert_eq!(mods.hotkey(0x0B, false), None, "0 não é máquina");
        mods.key(0x38, false, false);
        assert_eq!(mods.hotkey(0x02, false), None, "sem Alt não dispara");
    }

    #[test]
    fn texto_tem_prioridade_no_clipboard() {
        assert_eq!(clipboard_choice(true, true, true, true), ClipboardChoice::Text);
        assert_eq!(clipboard_choice(true, false, true, true), ClipboardChoice::Png);
        assert_eq!(
            clipboard_choice(true, false, false, true),
            ClipboardChoice::Dib,
            "imagem copiada no navegador vem com arquivo temporário: vale a imagem"
        );
        assert_eq!(
            clipboard_choice(true, false, false, false),
            ClipboardChoice::Files,
            "cópia no Explorer só tem arquivos"
        );
        assert_eq!(clipboard_choice(false, false, false, false), ClipboardChoice::None);
    }

    #[test]
    fn troca_de_desktop_nao_e_injetavel() {
        let mut injector = Injector::with_sender(record);
        assert!(injector.send(Event::DesktopSwitch).is_err());
    }

    #[test]
    fn program_data_vem_do_shell() {
        let dir = program_data_dir().expect("pasta conhecida");
        assert!(dir.ends_with("ProgramData"), "{}", dir.display());
    }

    #[test]
    fn elevacao_e_consultavel() {
        // Oráculo independente: `net session` só funciona em processo elevado (o runner da CI é).
        let elevated = std::process::Command::new("net")
            .arg("session")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("net session")
            .success();
        assert_eq!(is_elevated(), elevated);
    }

    #[test]
    fn eventos_da_bandeja_e_de_monitores_nao_sao_injetaveis() {
        let mut injector = Injector::with_sender(record);
        assert!(injector.send(Event::Tray(TrayCommand::Arrange)).is_err());
        assert!(injector.send(Event::DisplayChange).is_err());
    }

    #[test]
    fn dono_da_pasta() {
        assert!(
            owned_by_admins(std::path::Path::new(r"C:\ProgramData")).expect("ProgramData"),
            "ProgramData pertence ao SYSTEM"
        );
        let mine = std::env::temp_dir().join(format!("mousex-dono-{}", std::process::id()));
        std::fs::create_dir_all(&mine).expect("pasta");
        assert!(!owned_by_admins(&mine).expect("temp"), "pasta criada sem elevação pertence ao usuário");
        let _ = std::fs::remove_dir(&mine);
    }

    #[test]
    fn caminho_com_nome_curto_do_dos_nao_e_confundido_com_link() {
        let dir = std::env::temp_dir().join(format!("mousex-nome-bem-longo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("pasta");
        std::fs::write(dir.join("arquivo.txt"), "x").expect("arquivo");
        let short = {
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut buffer = vec![0u16; 1024];
            // SAFETY: caminho terminado em NUL; buffer do tamanho informado.
            let length = unsafe {
                windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                    wide.as_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                )
            } as usize;
            std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]))
        };
        assert!(open_unlinked(&short.join("arquivo.txt"), false).is_ok(), "{} é a mesma pasta", short.display());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pasta_protegida_nasce_sem_heranca_e_nao_reaproveita_existente() {
        let dir = std::env::temp_dir().join(format!("mousex-protegida-{}", std::process::id()));
        let _ = std::fs::remove_dir(&dir);
        create_protected_dir(&dir).expect("criar");
        let acl = std::process::Command::new("icacls").arg(&dir).output().expect("icacls");
        assert!(!String::from_utf8_lossy(&acl.stdout).contains("(I)"), "DACL protegida: nada herdado de ProgramData");
        assert_eq!(create_protected_dir(&dir).expect_err("já existe").kind(), io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn movimento_anterior_ao_park_nao_vira_delta() {
        assert_eq!(hook_move_at(1725, 718, true, (1720, 720), 100, 200), Event::Move { x: 1725, y: 718 });
        assert_eq!(hook_move_at(1725, 718, true, (1720, 720), 200, 200), Event::Delta { dx: 5, dy: -2 });
    }
}
