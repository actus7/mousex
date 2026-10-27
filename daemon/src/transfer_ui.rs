//! Avisos da cópia de arquivos recebida: a janela de progresso (com Cancelar, só se a cópia demorar) e o
//! aviso "cole com Ctrl+V" quando os arquivos já estão no clipboard. As janelas são da plataforma.
use crate::transfer_progress::{self, Progress, SHOW_AFTER};
use mousex_core::i18n::lang;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Tempo que o aviso "cole com Ctrl+V" fica na tela.
const NOTICE_FOR: Duration = Duration::from_secs(8);

/// Acompanha uma cópia em andamento: se ainda estiver rodando depois de `SHOW_AFTER`, abre a janela de progresso.
/// Cópias rápidas terminam antes e não mostram nada além do aviso final.
pub fn watch(progress: Arc<Progress>) {
    let started = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(SHOW_AFTER).await;
        if progress.is_finished() {
            return;
        }
        let cancel = progress.clone();
        let poll = progress.clone();
        let poll = Box::new(move || -> Option<(String, Option<u8>)> {
            if poll.is_finished() {
                return None;
            }
            let (done, total) = poll.snapshot();
            let text = format!("{}\n{}", poll.label(), transfer_progress::detail(done, total, started.elapsed()));
            Some((text, Some(transfer_progress::percent(done, total))))
        });
        let cancel_label: String = lang().pick("Cancelar", "Cancel").into();
        let cancel_action = Box::new(move || cancel.cancel());
        crate::platform::show_panel("MouseX", poll, Some((cancel_label, cancel_action)));
    });
}

/// Aviso no computador que recebeu: os arquivos já estão no clipboard, falta colar.
pub fn files_ready(count: usize) {
    let paste = if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" };
    let text = match count {
        1 => lang().pick(format!("1 arquivo pronto — cole com {paste}"), format!("1 file ready — paste with {paste}")),
        _ => lang().pick(
            format!("{count} arquivos prontos — cole com {paste}"),
            format!("{count} files ready — paste with {paste}"),
        ),
    };
    let started = Instant::now();
    let poll = Box::new(move || -> Option<(String, Option<u8>)> {
        (started.elapsed() < NOTICE_FOR).then(|| (text.clone(), None))
    });
    crate::platform::show_panel("MouseX", poll, None);
}
