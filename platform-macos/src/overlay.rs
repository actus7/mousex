//! Identificar telas no Mac: uma NSWindow sem borda por monitor, em nível alto e ignorando o mouse.
//! AppKit só cria janelas na thread principal, e o daemon ocupa essa thread com o tokio (block_on). Sem o laço
//! do AppKit rodando nela, a fila principal não é drenada: nada aparece, mas a injeção não espera por isso.
//! O trabalho vai para a fila principal do libdispatch (nunca bloqueia quem chama).
use crate::appkit_y;
use core_graphics::display::CGDisplay;
use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSProgressIndicator, NSTextAlignment, NSTextField, NSWindow, NSWindowStyleMask,
};
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

/// Atualização do painel: texto e percentual (None: sem barra). Devolver None fecha a janela.
pub type PanelPoll = Box<dyn Fn() -> Option<(String, Option<u8>)> + Send>;

/// Janela do painel e os controles que mudam, guardados entre um tick e o seguinte.
struct Panel {
    window: Retained<NSWindow>,
    text: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    poll: PanelPoll,
}

const PANEL_TICK_NANOS: i64 = 150_000_000;
const PANEL_WIDTH: f64 = 440.0;
const PANEL_HEIGHT: f64 = 150.0;
/// NSFloatingWindowLevel: acima das janelas comuns, sem ir para cima do protetor de tela.
const FLOATING_LEVEL: isize = 3;

/// Abre o painel de transferência na fila principal e volta na hora. `poll` roda ali a cada 150 ms.
// ponytail: sem botão Cancelar (precisa de classe ObjC com ação); o painel só mostra progresso e aviso.
pub fn show_panel(title: &str, poll: PanelPoll) {
    let job = Box::new((title.to_string(), poll));
    // SAFETY: a fila principal é um símbolo fixo do libdispatch; `open_panel` libera o contexto uma única vez.
    unsafe {
        let queue = std::ptr::addr_of!(_dispatch_main_q);
        dispatch_async_f(queue.cast(), Box::into_raw(job).cast(), open_panel);
    }
}

extern "C" fn open_panel(context: *mut c_void) {
    // SAFETY: `context` veio de `Box::into_raw` em `show_panel` e é consumido aqui, uma única vez.
    let (title, poll) = *unsafe { Box::from_raw(context.cast::<(String, PanelPoll)>()) };
    // Fora da thread principal o AppKit não cria janelas: a tarefa é descartada.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    schedule_tick(Box::new(Panel::open(mtm, &title, poll)));
}

fn schedule_tick(panel: Box<Panel>) {
    // SAFETY: relógio padrão do libdispatch; `tick` consome o contexto (e o devolve se o painel continuar).
    unsafe {
        let when = dispatch_time(DISPATCH_TIME_NOW, PANEL_TICK_NANOS);
        let queue = std::ptr::addr_of!(_dispatch_main_q);
        dispatch_after_f(when, queue.cast(), Box::into_raw(panel).cast(), tick);
    }
}

extern "C" fn tick(context: *mut c_void) {
    // SAFETY: veio de `Box::into_raw` em `schedule_tick`; consumido aqui e devolvido só se o painel continuar.
    let panel = unsafe { Box::from_raw(context.cast::<Panel>()) };
    match (panel.poll)() {
        None => panel.window.close(),
        Some((text, percent)) => {
            panel.text.setStringValue(&NSString::from_str(&text));
            panel.bar.setHidden(percent.is_none());
            if let Some(percent) = percent {
                panel.bar.setDoubleValue(f64::from(percent));
            }
            schedule_tick(panel);
        }
    }
}

impl Panel {
    /// Janela com título ao centro, com texto e barra; sem ativar o app (não rouba o foco).
    fn open(mtm: MainThreadMarker, title: &str, poll: PanelPoll) -> Panel {
        // SAFETY: estamos na thread principal (o MainThreadMarker garante) e a alocação acabou de ser feita aqui.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_WIDTH, PANEL_HEIGHT)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: a janela é guardada entre os ticks; sem isto, `close` a liberaria antes do `Retained` soltar.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(title));
        window.setLevel(FLOATING_LEVEL);
        window.center();
        let text = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        text.setFrame(NSRect::new(NSPoint::new(16.0, 96.0), NSSize::new(408.0, 40.0)));
        let bar = NSProgressIndicator::new(mtm);
        bar.setIndeterminate(false);
        bar.setMinValue(0.0);
        bar.setMaxValue(100.0);
        bar.setFrame(NSRect::new(NSPoint::new(16.0, 60.0), NSSize::new(408.0, 20.0)));
        if let Some(content) = window.contentView() {
            content.addSubview(&text);
            content.addSubview(&bar);
        }
        window.orderFrontRegardless();
        Panel { window, text, bar, poll }
    }
}
