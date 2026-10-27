//! Ícone na barra de menus (NSStatusItem) e a janela "Sobre" (NSAlert). Tudo na thread principal:
//! o AppKit só aceita menu e alerta nela. O estado do ícone fica numa thread-local, não num bloco do timer,
//! porque o bloco precisa ser `Send`.
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, runtime::Sel, sel, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, NSTimer};
use std::{cell::RefCell, ptr::NonNull};

/// De quanto em quanto tempo o rótulo de status é conferido (segundos).
const REFRESH_SECONDS: f64 = 1.0;

/// Ações do menu; ficam guardadas no próprio objeto que o AppKit chama.
struct Actions {
    on_log: Box<dyn Fn()>,
    on_about: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject não tem requisitos de subclasse, e a classe não implementa Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct StatusTarget;

    impl StatusTarget {
        #[unsafe(method(openLog:))]
        fn open_log(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_log)();
        }

        #[unsafe(method(showAbout:))]
        fn show_about_action(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_about)();
        }
    }

    unsafe impl NSObjectProtocol for StatusTarget {}
);

impl StatusTarget {
    fn new(mtm: MainThreadMarker, actions: Actions) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(actions);
        // SAFETY: `init` do NSObject em um objeto recém-alocado com ivars definidos.
        unsafe { msg_send![super(this), init] }
    }
}

/// O que muda com o tempo: o item de status e o rótulo mostrado da última vez.
struct Bar {
    status_row: Retained<NSMenuItem>,
    shown: String,
    status: Box<dyn Fn() -> String>,
    // Mantidos vivos: o item guarda o alvo sem retê-lo, e o ícone some se for solto.
    _item: Retained<NSStatusItem>,
    _target: Retained<StatusTarget>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Relê o status e troca o rótulo do menu só se mudou.
fn refresh() {
    BAR.with_borrow_mut(|bar| {
        let Some(bar) = bar else { return };
        let text = (bar.status)();
        if text != bar.shown {
            bar.status_row.setTitle(&NSString::from_str(&text));
            bar.shown = text;
        }
    });
}

fn action_row(mtm: MainThreadMarker, title: &str, action: Sel, target: &StatusTarget) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    // SAFETY: o seletor existe em StatusTarget, e o alvo continua vivo enquanto o menu existir (guardado em BAR).
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(target));
    }
    item
}

/// Mostra o ícone e roda o loop do AppKit (bloqueia). `status` é relido a cada segundo.
/// Sem Dock: a política Accessory deixa só o ícone da barra de menus.
pub fn run(
    status: impl Fn() -> String + 'static,
    log_label: &str,
    version_label: &str,
    on_log: impl Fn() + 'static,
    on_about: impl Fn() + 'static,
) {
    let mtm = MainThreadMarker::new().expect("status_item::run precisa da thread principal");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let target = StatusTarget::new(mtm, Actions { on_log: Box::new(on_log), on_about: Box::new(on_about) });
    let status_row = NSMenuItem::new(mtm);
    status_row.setTitle(&NSString::from_str(&status()));
    status_row.setEnabled(false);
    let log_row = action_row(mtm, log_label, sel!(openLog:), &target);
    let version_row = action_row(mtm, version_label, sel!(showAbout:), &target);

    let menu = NSMenu::new(mtm);
    menu.addItem(&status_row);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&log_row);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&version_row);

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        button.setTitle(&NSString::from_str("MouseX"));
    }
    item.setMenu(Some(&menu));

    let shown = status();
    BAR.with_borrow_mut(|bar| {
        *bar = Some(Bar { status_row, shown, status: Box::new(status), _item: item, _target: target });
    });
    // O bloco não captura nada (o estado está em BAR), então é seguro para o timer.
    let tick = RcBlock::new(|_timer: NonNull<NSTimer>| refresh());
    // SAFETY: o timer fica no run loop principal, que é esta thread.
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(REFRESH_SECONDS, true, &tick) };
    app.run();
}

/// Janela "Sobre" (NSAlert), modal. Chamar na thread principal, como as ações do menu.
pub fn show_about(title: &str, text: &str) {
    let mtm = MainThreadMarker::new().expect("show_about precisa da thread principal");
    // Sem isto, app Accessory fica atrás das outras janelas. `activate` só existe no macOS 14+;
    // esta chamada (depreciada) funciona em todas as versões suportadas.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    alert.runModal();
}
