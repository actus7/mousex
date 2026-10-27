//! Captura global de teclado e mouse por event tap do CoreGraphics, e o cursor parado enquanto o controle está
//! em outra máquina. O callback roda no run loop da thread que criou o tap e nunca espera nada.
//!
//! O tap é declarado aqui por FFI, e não pelo `core-graphics`, porque o wrapper dele não consegue descartar um
//! evento (devolve sempre o original). Descartar é devolver NULL no callback.
use crate::{macos::INPUT_MARKER, modifier_down, Input};
use core_foundation::{
    base::TCFType,
    mach_port::{CFMachPort, CFMachPortRef},
    runloop::{kCFRunLoopCommonModes, CFRunLoop},
};
use core_graphics::{
    display::CGDisplay,
    event::CGEvent,
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use std::{
    ffi::c_void,
    io, ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

type Handler = Box<dyn FnMut(Input) -> bool + Send>;
type TapCallback = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

// Valores de CGEventTapLocation, CGEventTapPlacement e CGEventTapOptions (tap ativo: pode engolir eventos).
const TAP_HID: u32 = 0;
const HEAD_INSERT: u32 = 0;
const ACTIVE_FILTER: u32 = 0;

// Tipos de evento (CGEventType).
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const MOVED: u32 = 5;
const LEFT_DRAGGED: u32 = 6;
const RIGHT_DRAGGED: u32 = 7;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const SCROLL: u32 = 22;
const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const OTHER_DRAGGED: u32 = 27;
const DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const TAP_EVENTS: [u32; 14] = [
    LEFT_DOWN,
    LEFT_UP,
    RIGHT_DOWN,
    RIGHT_UP,
    MOVED,
    LEFT_DRAGGED,
    RIGHT_DRAGGED,
    KEY_DOWN,
    KEY_UP,
    FLAGS_CHANGED,
    SCROLL,
    OTHER_DOWN,
    OTHER_UP,
    OTHER_DRAGGED,
];

// Campos de CGEventField.
const FIELD_BUTTON_NUMBER: u32 = 3;
const FIELD_DELTA_X: u32 = 4;
const FIELD_DELTA_Y: u32 = 5;
const FIELD_KEYCODE: u32 = 9;
const FIELD_USER_DATA: u32 = 42;
const FIELD_SCROLL_VERTICAL: u32 = 96;
const FIELD_SCROLL_HORIZONTAL: u32 = 97;

const CG_ERROR_SUCCESS: i32 = 0;

/// Porta do tap, para religar quando o sistema o desliga. Só o run loop do tap escreve nela.
static TAP: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
/// Tap criado e rodando: sem ele, suprimir o cursor travaria o mouse sem nenhum evento sendo engolido.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Controle em outra máquina: o tap engole as entradas e o cursor local fica parado.
static SUPPRESS: AtomicBool = AtomicBool::new(false);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetFlags(event: *mut c_void) -> u64;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// Cria o tap e roda o run loop desta thread (bloqueia). Erro se o macOS recusar (falta a permissão de
/// Monitoramento de Entrada ou de Acessibilidade); quem chama pode tentar de novo.
pub fn run(handler: impl FnMut(Input) -> bool + Send + 'static) -> io::Result<()> {
    let handler: Handler = Box::new(handler);
    // O tap dura o processo: o handler fica vivo de propósito (vazamento único, no início).
    let user = Box::into_raw(Box::new(handler)) as *mut c_void;
    let mask = TAP_EVENTS.iter().fold(0u64, |mask, &kind| mask | (1u64 << kind));
    // SAFETY: `callback` tem a assinatura que o CoreGraphics espera; `user` aponta para o handler acima.
    let port = unsafe { CGEventTapCreate(TAP_HID, HEAD_INSERT, ACTIVE_FILTER, mask, callback, user) };
    if port.is_null() {
        // SAFETY: sem tap nenhum callback usará `user`, então o handler pode ser liberado.
        drop(unsafe { Box::from_raw(user as *mut Handler) });
        return Err(io::Error::other(
            "macOS não criou o tap de entrada: falta permissão de Monitoramento de Entrada ou de Acessibilidade",
        ));
    }
    // SAFETY: `port` é um CFMachPortRef novo (regra Create); esta função passa a ser dona dele.
    let port = unsafe { CFMachPort::wrap_under_create_rule(port as CFMachPortRef) };
    let source = port.create_runloop_source(0).map_err(|_| io::Error::other("tap sem fonte de run loop"))?;
    // SAFETY: constante do framework CoreFoundation, válida durante todo o processo.
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    let raw = port.as_concrete_TypeRef() as *mut c_void;
    TAP.store(raw, Ordering::Release);
    // SAFETY: porta válida, criada e mantida viva acima.
    unsafe { CGEventTapEnable(raw, true) };
    ACTIVE.store(true, Ordering::Release);
    CFRunLoop::run_current();
    Ok(())
}

/// Se o macOS concede a permissão de Monitoramento de Entrada (sem pedir nada).
pub fn listen_access() -> bool {
    // SAFETY: função sem ponteiros.
    unsafe { CGPreflightListenEventAccess() }
}

/// Pede a permissão de Monitoramento de Entrada (o macOS mostra o aviso na primeira vez).
pub fn request_listen_access() -> bool {
    // SAFETY: função sem ponteiros.
    unsafe { CGRequestListenEventAccess() }
}

/// Liga ou desliga a supressão. Ligar falha se o tap não estiver rodando: aí o cursor não fica preso.
pub fn set_suppressed(on: bool) -> bool {
    if on && !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    // Desassociar faz o cursor ficar parado enquanto o mouse continua mandando deltas para o tap.
    let connected = !on;
    // SAFETY: a chamada é só do CoreGraphics (sem ponteiros do chamador).
    let ok = unsafe { CGAssociateMouseAndMouseCursorPosition(connected) } == CG_ERROR_SUCCESS;
    SUPPRESS.store(on && ok, Ordering::Release);
    ok
}

pub fn suppressed() -> bool {
    SUPPRESS.load(Ordering::Acquire)
}

/// Move o cursor (pontos globais, origem no canto superior esquerdo do monitor principal).
pub fn warp(x: f64, y: f64) {
    let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(x, y));
}

/// Estaciona o cursor no centro do monitor principal e devolve a posição real resultante.
pub fn park() -> (i32, i32) {
    let bounds = CGDisplay::main().bounds();
    warp(bounds.origin.x + bounds.size.width / 2.0, bounds.origin.y + bounds.size.height / 2.0);
    cursor_position()
}

fn cursor_position() -> (i32, i32) {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return (0, 0) };
    let Ok(event) = CGEvent::new(source) else { return (0, 0) };
    let at = event.location();
    (at.x.round() as i32, at.y.round() as i32)
}

unsafe extern "C" fn callback(_proxy: *mut c_void, kind: u32, event: *mut c_void, user: *mut c_void) -> *mut c_void {
    // SAFETY: `user` é o handler que `run` deixou vivo; só o run loop desta thread chama o callback.
    let handler = unsafe { &mut *(user as *mut Handler) };
    if kind == DISABLED_BY_TIMEOUT || kind == DISABLED_BY_USER_INPUT {
        // SAFETY: a porta foi guardada em `run` e vive enquanto o run loop roda.
        unsafe { CGEventTapEnable(TAP.load(Ordering::Acquire), true) };
        handler(Input::TapRestarted);
        return event;
    }
    // SAFETY: `event` é um CGEventRef válido durante o callback.
    if unsafe { CGEventGetIntegerValueField(event, FIELD_USER_DATA) } == INPUT_MARKER {
        return event;
    }
    // SAFETY: idem.
    let drop = unsafe { translate(kind, event) }.is_some_and(handler);
    if drop {
        ptr::null_mut()
    } else {
        event
    }
}

/// SAFETY: `event` é um CGEventRef válido durante o callback.
unsafe fn translate(kind: u32, event: *mut c_void) -> Option<Input> {
    let field = |name: u32| unsafe { CGEventGetIntegerValueField(event, name) };
    Some(match kind {
        LEFT_DOWN | LEFT_UP => Input::Button { button: 0, down: kind == LEFT_DOWN },
        RIGHT_DOWN | RIGHT_UP => Input::Button { button: 1, down: kind == RIGHT_DOWN },
        OTHER_DOWN | OTHER_UP => {
            let button = match field(FIELD_BUTTON_NUMBER) {
                number @ 2..=4 => number as u8,
                _ => return None,
            };
            Input::Button { button, down: kind == OTHER_DOWN }
        }
        MOVED | LEFT_DRAGGED | RIGHT_DRAGGED | OTHER_DRAGGED => {
            // SAFETY: mesmo evento do callback.
            let at = unsafe { CGEventGetLocation(event) };
            Input::Pointer { x: at.x, y: at.y, dx: field(FIELD_DELTA_X), dy: field(FIELD_DELTA_Y) }
        }
        KEY_DOWN | KEY_UP => Input::Key { keycode: field(FIELD_KEYCODE) as u16, down: kind == KEY_DOWN },
        FLAGS_CHANGED => {
            let keycode = field(FIELD_KEYCODE) as u16;
            // SAFETY: mesmo evento do callback.
            let flags = unsafe { CGEventGetFlags(event) };
            Input::Key { keycode, down: modifier_down(keycode, flags) }
        }
        SCROLL => Input::Scroll { vertical: field(FIELD_SCROLL_VERTICAL), horizontal: field(FIELD_SCROLL_HORIZONTAL) },
        _ => return None,
    })
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}
