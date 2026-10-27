//! Ícone na barra de menus (NSStatusItem) e a janela "Sobre" (NSAlert). Tudo na thread principal:
//! o AppKit só aceita menu e alerta nela. O menu fica numa thread-local, não num bloco do timer,
//! porque o bloco precisa ser `Send`.
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, NSTimer};
use std::{cell::RefCell, ptr::NonNull};

/// De quanto em quanto tempo as linhas do menu são relidas (segundos).
const REFRESH_SECONDS: f64 = 1.0;

/// Linha do menu. `Action` chama `on_pick` com o `id` ao ser escolhida; `Label` é só texto, desabilitado.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Label(String),
    Separator,
    Action { title: String, id: usize },
}

/// Ação escolhida no menu; fica guardada no próprio objeto que o AppKit chama.
struct Picker(Box<dyn Fn(usize)>);

define_class!(
    // SAFETY: NSObject não tem requisitos de subclasse, e a classe não implementa Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Picker]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(pick:))]
        fn pick(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender else { return };
            // SAFETY: o remetente é o NSMenuItem criado em `fill`, que tem `tag` (NSInteger).
            let tag: isize = unsafe { msg_send![item, tag] };
            (self.ivars().0)(tag as usize);
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, picker: Picker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(picker);
        // SAFETY: `init` do NSObject em um objeto recém-alocado com ivars definidos.
        unsafe { msg_send![super(this), init] }
    }
}

/// O que muda com o tempo: o menu, as linhas mostradas da última vez e como relê-las.
struct Bar {
    menu: Retained<NSMenu>,
    shown: Vec<Row>,
    rows: Box<dyn Fn() -> Vec<Row>>,
    // Mantidos vivos: o item guarda o alvo sem retê-lo, e o ícone some se for solto.
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Troca os itens do menu pelas linhas dadas. Cada item guarda o `id` na tag; o alvo recebe a escolha.
fn fill(menu: &NSMenu, rows: &[Row], target: &MenuTarget, mtm: MainThreadMarker) {
    menu.removeAllItems();
    for row in rows {
        let item = match row {
            Row::Separator => NSMenuItem::separatorItem(mtm),
            Row::Label(title) => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                item.setEnabled(false);
                item
            }
            Row::Action { title, id } => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                item.setTag(*id as isize);
                // SAFETY: o seletor existe em MenuTarget, e o alvo continua vivo enquanto o menu existir (em BAR).
                unsafe {
                    item.setAction(Some(sel!(pick:)));
                    item.setTarget(Some(target));
                }
                item
            }
        };
        menu.addItem(&item);
    }
}

/// Relê as linhas e refaz o menu só se mudaram.
fn refresh() {
    BAR.with_borrow_mut(|bar| {
        let Some(bar) = bar else { return };
        let rows = (bar.rows)();
        if rows != bar.shown {
            let mtm = MainThreadMarker::new().expect("refresh roda na thread principal");
            fill(&bar.menu, &rows, &bar._target, mtm);
            bar.shown = rows;
        }
    });
}

/// Mostra o ícone com o menu e roda o loop do AppKit (bloqueia). `rows` é relido a cada segundo; `on_pick`
/// recebe o `id` da linha escolhida. Sem Dock: a política Accessory deixa só o ícone da barra de menus.
pub fn run_menu(rows: impl Fn() -> Vec<Row> + 'static, on_pick: impl Fn(usize) + 'static) {
    let mtm = MainThreadMarker::new().expect("run_menu precisa da thread principal");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let target = MenuTarget::new(mtm, Picker(Box::new(on_pick)));
    let menu = NSMenu::new(mtm);
    let shown = rows();
    fill(&menu, &shown, &target, mtm);

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        button.setTitle(&NSString::from_str("MouseX"));
    }
    item.setMenu(Some(&menu));

    BAR.with_borrow_mut(|bar| {
        *bar = Some(Bar { menu, shown, rows: Box::new(rows), _item: item, _target: target });
    });
    // O bloco não captura nada (o estado está em BAR), então é seguro para o timer.
    let tick = RcBlock::new(|_timer: NonNull<NSTimer>| refresh());
    // SAFETY: o timer fica no run loop principal, que é esta thread.
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(REFRESH_SECONDS, true, &tick) };
    app.run();
}

const CLIENT_LOG: usize = 0;
const CLIENT_ABOUT: usize = 1;

/// Menu do cliente: status, abrir log e Sobre. `status` é relido a cada segundo.
pub fn run(
    status: impl Fn() -> String + 'static,
    log_label: &str,
    version_label: &str,
    on_log: impl Fn() + 'static,
    on_about: impl Fn() + 'static,
) {
    let (log_label, version_label) = (log_label.to_string(), version_label.to_string());
    run_menu(
        move || {
            vec![
                Row::Label(status()),
                Row::Separator,
                Row::Action { title: log_label.clone(), id: CLIENT_LOG },
                Row::Separator,
                Row::Action { title: version_label.clone(), id: CLIENT_ABOUT },
            ]
        },
        move |id| match id {
            CLIENT_LOG => on_log(),
            _ => on_about(),
        },
    );
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
