//! Identificar telas no Mac: uma NSWindow sem borda por monitor, em nível alto e ignorando o mouse.
//! AppKit só cria janelas na thread principal, e o daemon ocupa essa thread com o tokio (block_on). Sem o laço
//! do AppKit rodando nela, a fila principal não é drenada: nada aparece, mas a injeção não espera por isso.
//! O trabalho vai para a fila principal do libdispatch (nunca bloqueia quem chama).
use crate::appkit_y;
use core_graphics::display::CGDisplay;
use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSFont, NSTextAlignment, NSTextField, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::ffi::c_void;

const SHOW_FOR_NANOS: i64 = 3_000_000_000;
const DISPATCH_TIME_NOW: u64 = 0;
/// NSScreenSaverWindowLevel: acima de qualquer janela comum.
const SCREEN_SAVER_LEVEL: isize = 1000;

/// Monitor em pontos (Quartz: origem no topo à esquerda do principal) e o número que ele mostra.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdentifyScreen {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub number: u32,
}

/// Trabalho da fila principal: as telas e a altura do monitor principal, para a conversão de y.
struct Job {
    screens: Vec<IdentifyScreen>,
    main_height: f64,
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(when: u64, queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// Agenda as janelas na fila principal e volta na hora.
pub fn show_identify(screens: &[IdentifyScreen]) {
    if screens.is_empty() {
        return;
    }
    let job = Box::new(Job { screens: screens.to_vec(), main_height: CGDisplay::main().bounds().size.height });
    // SAFETY: a fila principal é um símbolo fixo do libdispatch; `show_on_main` libera o contexto uma única vez.
    unsafe {
        let queue = std::ptr::addr_of!(_dispatch_main_q);
        dispatch_async_f(queue.cast(), Box::into_raw(job).cast(), show_on_main);
    }
}

extern "C" fn show_on_main(context: *mut c_void) {
    // SAFETY: `context` veio de `Box::into_raw` em `show_identify` e é consumido aqui, uma única vez.
    let job = unsafe { Box::from_raw(context.cast::<Job>()) };
    // Fora da thread principal o AppKit não cria janelas: a tarefa é descartada.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let windows: Vec<Retained<NSWindow>> =
        job.screens.iter().map(|screen| window(mtm, screen, job.main_height)).collect();
    // SAFETY: o relógio padrão do libdispatch; `close_windows` libera o contexto uma única vez.
    unsafe {
        let when = dispatch_time(DISPATCH_TIME_NOW, SHOW_FOR_NANOS);
        let queue = std::ptr::addr_of!(_dispatch_main_q);
        dispatch_after_f(when, queue.cast(), Box::into_raw(Box::new(windows)).cast(), close_windows);
    }
}

extern "C" fn close_windows(context: *mut c_void) {
    // SAFETY: veio de `Box::into_raw` em `show_on_main`; liberado aqui, uma única vez.
    let windows = unsafe { Box::from_raw(context.cast::<Vec<Retained<NSWindow>>>()) };
    for window in windows.iter() {
        window.close();
    }
}

/// Janela sem borda cobrindo o monitor, com o número centralizado sobre fundo escuro translúcido.
fn window(mtm: MainThreadMarker, screen: &IdentifyScreen, main_height: f64) -> Retained<NSWindow> {
    let (width, height) = (screen.width as f64, screen.height as f64);
    let origin = NSPoint::new(screen.x as f64, appkit_y(screen.y as f64, height, main_height));
    // SAFETY: estamos na thread principal (o MainThreadMarker garante) e a alocação acabou de ser feita aqui.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(origin, NSSize::new(width, height)),
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: a janela é guardada no contexto do timer; sem isto, `close` a liberaria antes do `Retained` soltar.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setLevel(SCREEN_SAVER_LEVEL);
    window.setOpaque(false);
    window.setIgnoresMouseEvents(true);
    window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(0.06, 0.06, 0.06, 0.78)));
    let label = NSTextField::labelWithString(&NSString::from_str(&screen.number.to_string()), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(height * 0.4)));
    label.setTextColor(Some(&NSColor::whiteColor()));
    label.setAlignment(NSTextAlignment::Center);
    label.setFrame(NSRect::new(NSPoint::new(0.0, height * 0.25), NSSize::new(width, height * 0.5)));
    window.setContentView(Some(&label));
    window.orderFrontRegardless();
    window
}
