//! Windows: o processo que a tarefa agendada inicia só vigia o agente de verdade (um processo filho) e o
//! religa se ele cair. A tarefa não faz isso: "reiniciar em caso de falha" só vale para falha ao iniciar.
//! O filho fica num job que o encerra junto com o vigia (`schtasks /End`, atualização).
use crate::log;
use std::{
    process::Command,
    time::{Duration, Instant},
};

/// Argumento com que o vigia inicia o agente de verdade.
pub const WORKER: &str = "worker";
const RESTART_MIN: Duration = Duration::from_secs(1);
const RESTART_MAX: Duration = Duration::from_secs(30);
/// Uma sessão que durou isso conta como estável: a próxima queda religa rápido de novo.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// Espera antes de religar: rápido depois de rodar bem, dobrando em quedas seguidas (sem laço apertado).
fn restart_delay(ran: Duration, previous: Duration) -> Duration {
    if ran >= STABLE_RUN { RESTART_MIN } else { (previous * 2).min(RESTART_MAX) }
}

/// Vigia o agente até ele sair de propósito (código 0). Sem job (raro), ainda vigia, só sem a garantia.
pub fn supervise() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return log(&format!("supervisor: executable path unavailable: {error}")),
    };
    let job = mousex_native::KillOnCloseJob::new()
        .inspect_err(|error| log(&format!("supervisor: job object unavailable: {error}")))
        .ok();
    let mut delay = RESTART_MIN;
    loop {
        let started = Instant::now();
        let status = Command::new(&exe).arg(WORKER).spawn().and_then(|mut child| {
            if let Some(Err(error)) = job.as_ref().map(|job| job.assign(&child)) {
                log(&format!("supervisor: could not attach the agent to the job: {error}"));
            }
            child.wait()
        });
        match status {
            Ok(status) if status.success() => return,
            Ok(status) => log(&format!("agent stopped ({status}); restarting")),
            Err(error) => log(&format!("agent could not start ({error}); retrying")),
        }
        delay = restart_delay(started.elapsed(), delay);
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queda_depois_de_sessao_estavel_religa_rapido() {
        assert_eq!(restart_delay(Duration::from_secs(3600), Duration::from_secs(30)), RESTART_MIN);
    }

    #[test]
    fn quedas_seguidas_esperam_cada_vez_mais_ate_o_teto() {
        let mut delay = RESTART_MIN;
        let mut seen = Vec::new();
        for _ in 0..8 {
            delay = restart_delay(Duration::from_millis(200), delay);
            seen.push(delay.as_secs());
        }
        assert_eq!(seen, [2, 4, 8, 16, 30, 30, 30, 30]);
    }
}
