//! Painel de transferência: texto, barra opcional e botão Cancelar (ou só aviso, sem botão).
//! Janela própria numa thread, como identify.rs. A função `poll` roda a cada 150 ms na thread da janela e devolve
//! o texto e o percentual; quando devolve None, a janela fecha. Cancelar (botão ou X) roda a ação uma vez.
use std::{io, ptr, thread};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
};

/// Texto e percentual (None: sem barra) de cada atualização.
pub type Frame = (String, Option<u8>);
/// Atualização periódica; None fecha a janela.
pub type Poll = Box<dyn Fn() -> Option<Frame> + Send>;
/// Rótulo do botão e a ação dele (também roda quando o usuário fecha a janela pelo X).
pub type Cancel = (String, Box<dyn Fn() + Send>);

const TICK_MS: u32 = 150;
const TIMER_ID: usize = 1;
const CLASS: &str = "MouseXPanel";
const WIDTH: i32 = 440;
const HEIGHT: i32 = 170;
const BAR: RECT = RECT { left: 16, top: 62, right: 424, bottom: 80 };
const BUTTON: (i32, i32, i32, i32) = (300, 96, 124, 30);

/// COLORREF: 0x00BBGGRR.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
const TRACK_COLOR: u32 = rgb(214, 214, 214);
const FILL_COLOR: u32 = rgb(0, 120, 215);

/// Estado da janela, guardado em GWLP_USERDATA desde `run` até WM_DESTROY.
struct Panel {
    poll: Poll,
    cancel: Option<Box<dyn Fn() + Send>>,
    text: HWND,
    percent: Option<u8>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Estado da janela, ou None antes de `run` guardá-lo.
///
/// SAFETY: quem chama está na thread da janela; o ponteiro em GWLP_USERDATA vem de `Box::into_raw` em `run`
/// e só é liberado em WM_DESTROY, então, enquanto a janela existe, ele é válido.
unsafe fn panel_of<'a>(hwnd: HWND) -> Option<&'a mut Panel> {
    // SAFETY: GWLP_USERDATA é o ponteiro guardado por `run` (ou zero antes disso).
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Panel;
    // SAFETY: veja acima; `as_mut` devolve None para ponteiro nulo.
    unsafe { raw.as_mut() }
}

/// Botão ou X: roda a ação de cancelar no máximo uma vez e fecha a janela.
///
/// SAFETY: chamada só pela thread da janela, dentro do procedimento de janela.
unsafe fn cancel_and_close(hwnd: HWND) {
    // SAFETY: a thread da janela é a única que a acessa (ver `panel_of`).
    let cancel = unsafe { panel_of(hwnd) }.and_then(|panel| panel.cancel.take());
    if let Some(cancel) = cancel {
        cancel();
    }
    // SAFETY: janela criada nesta thread e ainda viva (o procedimento está rodando nela).
    unsafe { DestroyWindow(hwnd) };
}

/// Barra de progresso desenhada à mão: trilha cinza e preenchimento azul até o percentual.
///
/// SAFETY: `dc` é o DC válido do WM_PAINT em curso; os pincéis são criados e destruídos aqui.
unsafe fn draw_bar(dc: HDC, percent: u8) {
    let width = BAR.right - BAR.left;
    let fill = RECT { right: BAR.left + width * i32::from(percent.min(100)) / 100, ..BAR };
    // SAFETY: pincéis criados e destruídos neste bloco, dentro do WM_PAINT que recebeu `dc`.
    unsafe {
        let track = CreateSolidBrush(TRACK_COLOR);
        FillRect(dc, &BAR, track);
        DeleteObject(track);
        let color = CreateSolidBrush(FILL_COLOR);
        FillRect(dc, &fill, color);
        DeleteObject(color);
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: as mensagens desta janela chegam na thread que a criou; `panel_of` trata o ponteiro nulo.
    unsafe {
        match message {
            WM_TIMER => {
                if let Some(panel) = panel_of(hwnd) {
                    match (panel.poll)() {
                        // Fecha: WM_DESTROY libera o estado. Não usamos `panel` depois disso.
                        None => {
                            DestroyWindow(hwnd);
                        }
                        Some((text, percent)) => {
                            let text = wide(&text.replace('\n', "\r\n"));
                            SetWindowTextW(panel.text, text.as_ptr());
                            panel.percent = percent;
                            InvalidateRect(hwnd, ptr::null(), 1);
                        }
                    }
                }
                0
            }
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                if let Some(percent) = panel_of(hwnd).and_then(|panel| panel.percent) {
                    draw_bar(dc, percent);
                }
                EndPaint(hwnd, &paint);
                0
            }
            // Botão: notificação BN_CLICKED, com o HWND do botão em lparam.
            WM_COMMAND if lparam != 0 && (wparam >> 16) as u32 == BN_CLICKED => {
                cancel_and_close(hwnd);
                0
            }
            WM_CLOSE => {
                cancel_and_close(hwnd);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, TIMER_ID);
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !raw.is_null() {
                    // SAFETY: veio de Box::into_raw em `run`; zerado logo acima, então não é liberado duas vezes.
                    drop(Box::from_raw(raw));
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// Laço próprio da janela: o procedimento cuida de atualizar, cancelar e fechar.
fn run(title: &str, poll: Poll, cancel: Option<Cancel>) {
    let class = wide(CLASS);
    let title = wide(title);
    let (label, action) = match cancel {
        Some((label, action)) => (Some(label), Some(action)),
        None => (None, None),
    };
    // SAFETY: janela, controles e registro da classe ficam nesta thread; o Box<Panel> passa à janela e é liberado em WM_DESTROY.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(panel_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        wc.hbrBackground = (COLOR_WINDOW + 1) as usize as HBRUSH;
        // Já registrada por um painel anterior: o CreateWindowExW abaixo continua funcionando.
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIDTH,
            HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let text_class = wide("STATIC");
        let empty = wide("");
        let text = CreateWindowExW(
            0,
            text_class.as_ptr(),
            empty.as_ptr(),
            WS_CHILD | WS_VISIBLE,
            16,
            12,
            408,
            44,
            hwnd,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if let Some(label) = label {
            let button_class = wide("BUTTON");
            let label = wide(&label);
            let (x, y, w, h) = BUTTON;
            CreateWindowExW(
                0,
                button_class.as_ptr(),
                label.as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32,
                x,
                y,
                w,
                h,
                hwnd,
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
        }
        let panel = Box::new(Panel { poll, cancel: action, text, percent: None });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(panel) as isize);
        SetTimer(hwnd, TIMER_ID, TICK_MS, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// Abre o painel numa thread própria e volta na hora: a recepção nunca espera pela janela.
pub fn show_panel(title: &str, poll: Poll, cancel: Option<Cancel>) -> io::Result<()> {
    let title = title.to_string();
    thread::Builder::new().name("mousex-panel".into()).spawn(move || run(&title, poll, cancel))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    };
    use std::time::Duration;

    /// Teste visual, manual: abre o painel de verdade. Aperta o Cancelar (BM_CLICK) numa janela e o X (WM_CLOSE)
    /// em outra, e confere que as duas ações rodaram. Para tirar print, rodar com `--nocapture` e capturar a tela
    /// enquanto a janela está aberta (~2 s).
    #[test]
    #[ignore = "abre janelas de verdade"]
    fn botao_cancelar_e_x_disparam_a_acao() {
        for (title, by_button) in [("MouseX teste botao", true), ("MouseX teste x", false)] {
            let ticks = Arc::new(AtomicU32::new(0));
            let cancelled = Arc::new(AtomicBool::new(false));
            let counter = ticks.clone();
            let poll: Poll = Box::new(move || {
                let n = counter.fetch_add(1, Ordering::Relaxed).min(100);
                Some((format!("arquivo.bin\n{n}% · 3.2 MB de 7.5 MB · 12.0 MB/s"), Some(n as u8)))
            });
            let flag = cancelled.clone();
            let cancel: Cancel = ("Cancelar".into(), Box::new(move || flag.store(true, Ordering::Relaxed)));
            show_panel(title, poll, Some(cancel)).expect("thread da janela");
            std::thread::sleep(Duration::from_millis(2000));
            let wide_title = wide(title);
            // SAFETY: só leitura de handles de janelas do próprio processo; mensagens postadas (não enviadas) para a
            // thread da janela, que está no laço de mensagens.
            unsafe {
                let hwnd = FindWindowW(ptr::null(), wide_title.as_ptr());
                assert!(!hwnd.is_null(), "janela {title} aberta");
                if by_button {
                    let button = FindWindowExW(hwnd, ptr::null_mut(), wide("BUTTON").as_ptr(), ptr::null());
                    PostMessageW(button, BM_CLICK, 0, 0);
                } else {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
            std::thread::sleep(Duration::from_millis(500));
            assert!(cancelled.load(Ordering::Relaxed), "{title}: a ação de cancelar rodou");
        }
    }
}
