//! Identificar telas: um número grande no centro de cada monitor, por alguns segundos.
//! Janelas em camada (sem borda, sempre no topo, sem foco e sem clique) numa thread própria, desenhadas com GDI.
//! O número vai no título da janela, assim o procedimento de janela não guarda estado.
use std::{io, ptr, thread, time::Duration};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
};

const SHOW_FOR: Duration = Duration::from_secs(3);
/// Opacidade da janela inteira (texto incluído): o fundo escuro fica translúcido e o número continua legível.
const WINDOW_ALPHA: u8 = 200;
const CLASS: &str = "MouseXIdentify";

/// Retângulo físico de um monitor e o número que ele mostra.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdentifyScreen {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub number: u32,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// COLORREF: 0x00BBGGRR.
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Desenha o fundo escuro e o número centralizado, em fonte proporcional à altura do monitor.
unsafe extern "system" fn overlay_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message != WM_PAINT {
        // SAFETY: repasse padrão da mensagem.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    // SAFETY: dentro do WM_PAINT o DC é válido até EndPaint; os objetos GDI criados aqui são destruídos aqui.
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut area: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut area);
        let background = CreateSolidBrush(rgb(16, 16, 16));
        FillRect(dc, &area, background);
        DeleteObject(background);
        let mut title = [0u16; 16];
        let length = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        let font = CreateFontW(
            -(area.bottom * 6 / 10),
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
            ptr::null(),
        );
        let old = SelectObject(dc, font);
        SetTextColor(dc, rgb(255, 255, 255));
        SetBkMode(dc, TRANSPARENT as i32);
        DrawTextW(dc, title.as_ptr(), length, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(dc, old);
        DeleteObject(font);
        EndPaint(hwnd, &paint);
    }
    0
}

unsafe extern "system" fn stop_proc(_hwnd: HWND, _message: u32, _id: usize, _time: u32) {
    // SAFETY: chamada simples na thread do overlay; encerra o laço de mensagens.
    unsafe { PostQuitMessage(0) };
}

/// Cria e mostra uma janela por monitor, sem ativar nem roubar o foco.
fn create(instance: HMODULE, class: &[u16], screen: &IdentifyScreen) -> HWND {
    let title = wide(&screen.number.to_string());
    // SAFETY: classe registrada nesta thread; strings vivas durante a chamada.
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            screen.left,
            screen.top,
            screen.width,
            screen.height,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if !hwnd.is_null() {
            SetLayeredWindowAttributes(hwnd, 0, WINDOW_ALPHA, LWA_ALPHA);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        hwnd
    }
}

/// Laço próprio: o timer encerra a mensagem de saída e as janelas são destruídas na mesma thread.
fn run(screens: &[IdentifyScreen]) {
    let class = wide(CLASS);
    // SAFETY: registro e destruição das janelas ficam nesta thread; GetModuleHandleW(null) é o próprio executável.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(overlay_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        // Já registrada por um overlay anterior: o CreateWindowExW abaixo continua funcionando.
        RegisterClassW(&wc);
        let windows: Vec<HWND> =
            screens.iter().map(|screen| create(instance, &class, screen)).filter(|hwnd| !hwnd.is_null()).collect();
        SetTimer(ptr::null_mut(), 0, SHOW_FOR.as_millis() as u32, Some(stop_proc));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
        for hwnd in windows {
            DestroyWindow(hwnd);
        }
    }
}

/// Mostra os números numa thread própria e volta na hora: a injeção nunca espera por isso.
pub fn show_identify(screens: &[IdentifyScreen]) -> io::Result<()> {
    if screens.is_empty() {
        return Ok(());
    }
    let screens = screens.to_vec();
    thread::Builder::new().name("mousex-identify".into()).spawn(move || run(&screens))?;
    Ok(())
}
