use super::*;
use core_graphics::{
    display::CGDisplay,
    event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard};
use objc2_foundation::{NSData, NSDictionary, NSString};
use std::io;

/// Marca eventos injetados pelo MouseX.
pub const INPUT_MARKER: i64 = 0x4d58524d;
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CLIPBOARD_TEXT_BYTES: usize = 10 * 1024 * 1024;

fn err(message: &str) -> io::Error {
    io::Error::other(message.to_string())
}

fn source() -> io::Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| err("CGEventSource falhou"))
}

/// Monitor em pontos (coordenadas globais do Quartz, origem no canto superior esquerdo do principal).
#[derive(Debug, Clone)]
pub struct Display {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Pixels físicos por ponto (2.0 em Retina).
    pub scale: f64,
}

pub fn displays() -> Vec<Display> {
    let ids = CGDisplay::active_displays().unwrap_or_default();
    ids.into_iter()
        .map(|id| {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();
            let scale = display
                .display_mode()
                .map(|mode| mode.pixel_width() as f64 / mode.width().max(1) as f64)
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(1.0);
            Display {
                id,
                x: bounds.origin.x,
                y: bounds.origin.y,
                width: bounds.size.width,
                height: bounds.size.height,
                scale,
            }
        })
        .collect()
}

/// Injeta entrada mantendo estado próprio: modificadores, botões pressionados e contagem de cliques.
#[derive(Default)]
pub struct Injector {
    keys: std::collections::BTreeSet<u16>,
    buttons: std::collections::BTreeSet<u8>,
    modifiers: Modifiers,
    clicks: Clicks,
    position: (f64, f64),
}

fn mouse_button(button: u8) -> Option<(CGMouseButton, CGEventType, CGEventType, CGEventType)> {
    Some(match button {
        0 => (CGMouseButton::Left, CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGEventType::LeftMouseDragged),
        1 => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::RightMouseDragged,
        ),
        2..=4 => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
        ),
        _ => return None,
    })
}

impl Injector {
    fn post(&self, event: CGEvent, extra_flags: u64) {
        event.set_flags(CGEventFlags::from_bits_truncate(self.modifiers.flags() | extra_flags));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INPUT_MARKER);
        event.post(CGEventTapLocation::HID);
    }

    pub fn key(&mut self, keycode: u16, down: bool) -> io::Result<()> {
        let event = CGEvent::new_keyboard_event(source()?, keycode, down).map_err(|_| err("evento de teclado"))?;
        self.modifiers.key(keycode, down);
        if modifier_flag(keycode).is_some() {
            event.set_type(CGEventType::FlagsChanged);
        }
        if down {
            self.keys.insert(keycode);
        } else {
            self.keys.remove(&keycode);
        }
        self.post(event, key_flags(keycode));
        Ok(())
    }

    /// Posição absoluta em pontos globais; com botão pressionado vira "dragged" (arrastar).
    pub fn move_to(&mut self, x: f64, y: f64) -> io::Result<()> {
        self.position = (x, y);
        let (kind, button) = match self.buttons.iter().next().and_then(|&b| mouse_button(b)) {
            Some((button, _, _, dragged)) => (dragged, button),
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        };
        let event = CGEvent::new_mouse_event(source()?, kind, CGPoint::new(x, y), button)
            .map_err(|_| err("evento de mouse"))?;
        self.post(event, 0);
        Ok(())
    }

    pub fn button(&mut self, button: u8, down: bool) -> io::Result<()> {
        let (cg_button, down_type, up_type, _) = mouse_button(button).ok_or_else(|| err("botão inválido"))?;
        let count = if down { self.clicks.press(button, self.position, Instant::now()) } else { self.clicks.current() };
        let (x, y) = self.position;
        let event =
            CGEvent::new_mouse_event(source()?, if down { down_type } else { up_type }, CGPoint::new(x, y), cg_button)
                .map_err(|_| err("evento de botão"))?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, count);
        if button >= 2 {
            event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, button as i64);
        }
        if down {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        self.post(event, 0);
        Ok(())
    }

    /// Deltas de roda do Windows (120 por dente; positivo = para cima/direita).
    pub fn scroll(&mut self, dx: i32, dy: i32) -> io::Result<()> {
        let event =
            CGEvent::new_scroll_event(source()?, ScrollEventUnit::PIXEL, 2, scroll_pixels(dy), -scroll_pixels(dx), 0)
                .map_err(|_| err("evento de scroll"))?;
        self.post(event, 0);
        Ok(())
    }

    /// Print Screen: dispara o atalho de captura do macOS. O Shift do usuário, se houver, já está em `modifiers`.
    pub fn print_screen(&mut self) -> io::Result<()> {
        let shift_held = self.modifiers.held().iter().any(|&k| modifier_flag(k) == Some(FLAG_SHIFT));
        for (keycode, down) in print_screen_sequence(shift_held) {
            self.key(keycode, down)?;
        }
        Ok(())
    }

    /// Solta tudo que foi injetado e zera os modificadores (nenhuma tecla presa após queda).
    pub fn release_all(&mut self) {
        for button in self.buttons.clone() {
            let _ = self.button(button, false);
        }
        for keycode in self.keys.clone() {
            let _ = self.key(keycode, false);
        }
        self.modifiers.clear();
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionDeclareUserActivity(name: core_foundation::string::CFStringRef, kind: u32, id: *mut u32) -> i32;
}

/// `CGEventRef` com a codificação Objective-C certa (`^{__CGEvent=}`): o objc2 confere o retorno em build debug.
#[repr(C)]
struct OpaqueCGEvent {
    _private: [u8; 0],
}

// SAFETY: tipo opaco usado só por ponteiro; a codificação é a do struct __CGEvent declarado pelo CoreGraphics.
unsafe impl objc2::encode::RefEncode for OpaqueCGEvent {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__CGEvent", &[]));
}

/// Tecla de mídia ou volume (`NX_KEYTYPE_*`). O macOS só as reconhece como evento "system defined"
/// (subtipo 8), não como keycode: é o mesmo evento que o teclado da Apple gera.
pub fn media_key(key: u32, down: bool) -> io::Result<()> {
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::NSPoint;
    let state: isize = if down { 0xa } else { 0xb };
    let data1 = ((key as isize) << 16) | (state << 8);
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: seletor e tipos de argumento da API pública de NSEvent; o evento é autoliberado no pool,
        // e o CGEvent dele vale enquanto o NSEvent existe.
        unsafe {
            let event: *mut AnyObject = msg_send![class!(NSEvent),
                otherEventWithType: 14usize,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: (state as usize) << 8,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: std::ptr::null_mut::<AnyObject>(),
                subtype: 8i16,
                data1: data1,
                data2: -1isize];
            if event.is_null() {
                return Err(err("NSEvent de mídia não criado"));
            }
            let cg: *mut OpaqueCGEvent = msg_send![event, CGEvent];
            if cg.is_null() {
                return Err(err("CGEvent de mídia não criado"));
            }
            CGEventPost(0, cg.cast());
            Ok(())
        }
    })
}

/// Conta como atividade do usuário: tela não apaga nem bloqueia por inatividade agora. A política de
/// bloqueio continua valendo: sem novas chamadas (usuário longe), o Mac bloqueia no tempo normal.
pub fn declare_user_activity() -> bool {
    use core_foundation::{base::TCFType, string::CFString};
    static ASSERTION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let name = CFString::new("MouseX: usuário ativo em outra máquina");
    let mut id = ASSERTION.load(std::sync::atomic::Ordering::Relaxed);
    // SAFETY: string CF válida; `id` reaproveita a mesma asserção entre chamadas (kIOPMUserActiveLocal = 0).
    let status = unsafe { IOPMAssertionDeclareUserActivity(name.as_concrete_TypeRef(), 0, &mut id) };
    ASSERTION.store(id, std::sync::atomic::Ordering::Relaxed);
    status == 0
}

/// Permissão de Acessibilidade; com `prompt`, o macOS mostra o pedido ao usuário.
pub fn ensure_accessibility(prompt: bool) -> bool {
    use core_foundation::{base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString};
    // SAFETY: constante exportada pelo framework, válida durante o processo.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let value = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
    // SAFETY: dicionário CF válido.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    Text(String),
    Png(Vec<u8>),
    /// Arquivos e pastas copiados no Finder (caminhos absolutos).
    Files(Vec<std::path::PathBuf>),
}

/// Caminhos dos arquivos copiados no Finder. Cada item traz uma URL de arquivo (às vezes de referência,
/// "file:///.file/id=…", convertida para o caminho real).
fn clipboard_files(pasteboard: &NSPasteboard) -> Option<Vec<std::path::PathBuf>> {
    use objc2_foundation::NSURL;
    let items = pasteboard.pasteboardItems()?;
    let kind = NSString::from_str("public.file-url");
    let mut paths = Vec::new();
    for item in items.iter() {
        let Some(text) = item.stringForType(&kind) else { continue };
        let Some(url) = NSURL::URLWithString(&text) else { continue };
        let path = url.filePathURL().and_then(|file| file.path());
        if let Some(path) = path {
            paths.push(std::path::PathBuf::from(path.to_string()));
        }
    }
    (!paths.is_empty()).then_some(paths)
}

pub fn clipboard_seq() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

fn pasteboard_data(pasteboard: &NSPasteboard, kind: &str) -> Option<Vec<u8>> {
    pasteboard.dataForType(&NSString::from_str(kind)).map(|data| data.to_vec())
}

/// Arquivos primeiro (o Finder publica também o nome como texto e o ícone como imagem); depois texto
/// (planilhas publicam texto e imagem); depois PNG; depois TIFF (prints do macOS) convertido.
pub fn clipboard_read() -> Option<Clip> {
    let pasteboard = NSPasteboard::generalPasteboard();
    if let Some(paths) = clipboard_files(&pasteboard) {
        return Some(Clip::Files(paths));
    }
    if let Some(text) = pasteboard.stringForType(&NSString::from_str("public.utf8-plain-text")) {
        let text = text.to_string();
        return (text.len() <= MAX_CLIPBOARD_TEXT_BYTES).then_some(Clip::Text(text));
    }
    let png = pasteboard_data(&pasteboard, "public.png").or_else(|| {
        let tiff = pasteboard.dataForType(&NSString::from_str("public.tiff"))?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
        // SAFETY: dicionário vazio de propriedades, tipo PNG válido.
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        Some(png.to_vec())
    })?;
    (png.len() <= MAX_CLIPBOARD_IMAGE_BYTES).then_some(Clip::Png(png))
}

pub fn clipboard_write(clip: &Clip) -> io::Result<()> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    let ok = match clip {
        Clip::Text(text) => {
            pasteboard.setString_forType(&NSString::from_str(text), &NSString::from_str("public.utf8-plain-text"))
        }
        Clip::Png(png) => pasteboard.setData_forType(Some(&NSData::with_bytes(png)), &NSString::from_str("public.png")),
        Clip::Files(paths) => {
            use objc2_foundation::{NSArray, NSURL};
            let urls: Vec<_> =
                paths.iter().map(|path| NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))).collect();
            let array = NSArray::from_retained_slice(&urls);
            // SAFETY: NSArray de NSURL; NSURL implementa NSPasteboardWriting.
            unsafe { objc2::msg_send![&*pasteboard, writeObjects: &*array] }
        }
    };
    if ok {
        Ok(())
    } else {
        Err(err("NSPasteboard recusou a gravação"))
    }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyInputSourceID: core_foundation::string::CFStringRef;
    fn TISCopyCurrentKeyboardInputSource() -> *const std::ffi::c_void;
    fn TISGetInputSourceProperty(
        source: *const std::ffi::c_void,
        key: core_foundation::string::CFStringRef,
    ) -> *const std::ffi::c_void;
}

/// Identificador do layout de teclado ativo (ex.: "com.apple.keylayout.Brazilian-ABNT2").
/// Chamar na thread principal: as APIs de entrada de texto do macOS esperam isso.
pub fn input_source_id() -> Option<String> {
    use core_foundation::{
        base::{CFRelease, TCFType},
        string::CFString,
    };
    // SAFETY: fonte copiada (Copy Rule) e liberada; a propriedade segue a Get Rule (não liberar).
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return None;
        }
        let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) as core_foundation::string::CFStringRef;
        let text = (!id.is_null()).then(|| CFString::wrap_under_get_rule(id).to_string());
        CFRelease(source);
        text
    }
}

/// Notificação discreta da Central de Notificações.
pub fn notify(title: &str, text: &str) {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\"", quote(text), quote(title));
    let _ = std::process::Command::new("osascript").args(["-e", &script]).output();
}

/// Diálogo Sim/Não (padrão "Não") via AppleScript; bloqueia até a resposta.
pub fn ask_yes_no(title: &str, text: &str) -> bool {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display dialog \"{}\" with title \"{}\" buttons {{\"Não\", \"Sim\"}} default button \"Não\" cancel button \"Não\" with icon caution",
        quote(text),
        quote(title)
    );
    std::process::Command::new("osascript").args(["-e", &script]).output().is_ok_and(|out| out.status.success())
}
