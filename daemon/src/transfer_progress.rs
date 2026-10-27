//! Progresso de uma cópia de arquivos recebida: bytes, porcentagem, velocidade e quando mostrar a janela.
//! Só contas e estado atômico; quem desenha a janela é a plataforma (ver `transfer_ui.rs`).
use mousex_core::i18n::lang;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;

/// Depois deste tempo a janela de progresso aparece; cópias mais rápidas só mostram o aviso.
pub const SHOW_AFTER: Duration = Duration::from_secs(1);

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Estado de uma cópia em andamento, compartilhado entre a recepção e a janela.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
    finished: AtomicBool,
    label: Mutex<String>,
    wake: Notify,
}

impl Progress {
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Release);
    }

    pub fn advance(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::AcqRel);
    }

    pub fn snapshot(&self) -> (u64, u64) {
        (self.done.load(Ordering::Acquire), self.total.load(Ordering::Acquire))
    }

    pub fn set_label(&self, label: String) {
        // Mutex envenenado só acontece se a janela entrar em pânico; o rótulo antigo ainda serve.
        *self.label.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = label;
    }

    pub fn label(&self) -> String {
        self.label.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // notify_one guarda a permissão: o cancelamento não se perde se ninguém estiver esperando naquele instante.
        self.wake.notify_one();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Espera até alguém cancelar (retorna na hora se já cancelado).
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            self.wake.notified().await;
        }
    }

    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// Porcentagem inteira, sem passar de 100; sem total conhecido ainda, zero.
pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    (done.min(total) * 100 / total) as u8
}

/// Média desde o início da cópia, em bytes por segundo (zero se ainda não há tempo decorrido).
pub fn bytes_per_sec(done: u64, elapsed: Duration) -> u64 {
    let millis = elapsed.as_millis();
    if millis == 0 {
        return 0;
    }
    (done as u128 * 1000 / millis) as u64
}

/// Rótulo da cópia: o nome do item (um só) ou a quantidade de itens, em pt ou en.
pub fn label(roots: &[String]) -> String {
    match roots {
        [only] => only.clone(),
        _ => lang().pick(format!("{} arquivos", roots.len()), format!("{} files", roots.len())),
    }
}

/// Texto da janela: "42% · 3.2 MB de 7.5 MB · 12.0 MB/s".
pub fn detail(done: u64, total: u64, elapsed: Duration) -> String {
    format!("{}% · {} de {} · {}/s", percent(done, total), size(done), size(total), size(bytes_per_sec(done, elapsed)))
}

fn size(bytes: u64) -> String {
    if bytes >= BYTES_PER_MB {
        format!("{:.1} MB", bytes as f64 / BYTES_PER_MB as f64)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcentagem_sem_total_e_zero() {
        assert_eq!(percent(10, 0), 0);
    }

    #[test]
    fn porcentagem_nunca_passa_de_cem() {
        assert_eq!(percent(5, 4), 100);
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(2, 3), 66);
    }

    #[test]
    fn velocidade_sem_tempo_decorrido_nao_divide_por_zero() {
        assert_eq!(bytes_per_sec(1000, Duration::ZERO), 0);
        assert_eq!(bytes_per_sec(2000, Duration::from_secs(2)), 1000);
    }

    #[test]
    fn janela_so_aparece_depois_de_um_segundo() {
        assert_eq!(SHOW_AFTER, Duration::from_secs(1));
    }

    #[test]
    fn rotulo_usa_nome_de_um_item_e_contagem_de_varios() {
        assert_eq!(label(&["foto.png".to_string()]), "foto.png");
        let many = lang().pick("2 arquivos", "2 files");
        assert_eq!(label(&["a".to_string(), "b".to_string()]), many);
    }

    #[test]
    fn progresso_acumula_e_cancela_sem_perder_o_total() {
        let progress = Progress::default();
        progress.set_total(4 * BYTES_PER_MB);
        progress.advance(BYTES_PER_MB);
        progress.advance(BYTES_PER_MB);
        assert_eq!(progress.snapshot(), (2 * BYTES_PER_MB, 4 * BYTES_PER_MB));
        assert!(!progress.is_cancelled());
        progress.cancel();
        assert!(progress.is_cancelled());
        assert!(!progress.is_finished());
        progress.finish();
        assert!(progress.is_finished());
    }

    #[test]
    fn texto_traz_porcentagem_tamanhos_e_velocidade() {
        let text = detail(2 * BYTES_PER_MB, 4 * BYTES_PER_MB, Duration::from_secs(1));
        assert_eq!(text, "50% · 2.0 MB de 4.0 MB · 2.0 MB/s");
    }
}
