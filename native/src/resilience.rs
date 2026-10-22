//! Recuperação sozinha: hooks que o Windows removeu em silêncio e agente que caiu.
use std::{io, mem, os::windows::io::AsRawHandle, process::Child, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

/// Intervalo da checagem de vida dos hooks (timer na thread deles).
pub(crate) const HOOK_CHECK_MS: u32 = 2000;

/// O Windows remove um hook de baixo nível que demora demais, sem avisar. Sinal disso: o cursor anda,
/// mas o hook não vê nada. Dois intervalos seguidos assim (um salto só pode ser SetCursorPos nosso).
#[derive(Default)]
pub(crate) struct Liveness {
    last: Option<(i32, i32)>,
    suspect: bool,
}

impl Liveness {
    /// `now` e `last_hook` em ms do GetTickCount (que dá a volta a cada 49 dias). `true`: reinstalar.
    pub(crate) fn observe(&mut self, cursor: (i32, i32), now: u32, last_hook: u32) -> bool {
        let moved = self.last.is_some_and(|last| last != cursor);
        self.last = Some(cursor);
        let silent = now.wrapping_sub(last_hook) >= HOOK_CHECK_MS;
        let dead = moved && silent && self.suspect;
        self.suspect = moved && silent && !dead;
        dead
    }
}

/// Job que encerra os processos dele quando o último handle fecha, inclusive se o dono for morto
/// (`schtasks /End`, atualização): o agente filho nunca fica órfão segurando portas e arquivos.
pub struct KillOnCloseJob(HANDLE);

impl KillOnCloseJob {
    pub fn new() -> io::Result<Self> {
        // SAFETY: atributos e nome nulos são aceitos; a estrutura de limites é local e do tamanho informado.
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &limits as *const _ as _, size) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: handle do job válido enquanto `self` vive; o do filho, enquanto `child` vive.
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: handle próprio, fechado uma vez.
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: u32 = HOOK_CHECK_MS;

    #[test]
    fn cursor_andando_sem_evento_do_hook_por_dois_intervalos_reinstala() {
        let mut live = Liveness::default();
        assert!(!live.observe((0, 0), 0, 0), "primeira leitura só registra");
        assert!(!live.observe((50, 0), TICK, 0), "um intervalo suspeito ainda não basta");
        assert!(live.observe((90, 10), 2 * TICK, 0), "dois seguidos: hook morto");
        assert!(!live.observe((95, 10), 3 * TICK, 0), "depois de reinstalar, recomeça a contagem");
    }

    #[test]
    fn salto_unico_do_cursor_feito_pelo_programa_nao_dispara() {
        let mut live = Liveness::default();
        live.observe((0, 0), 0, 0);
        assert!(!live.observe((1720, 720), TICK, 0), "estacionar o cursor (SetCursorPos) não passa pelo hook");
        assert!(!live.observe((1720, 720), 2 * TICK, 0), "parado de novo: zera a suspeita");
        assert!(!live.observe((1800, 720), 3 * TICK, 0));
    }

    #[test]
    fn hook_vivo_nunca_dispara() {
        let mut live = Liveness::default();
        live.observe((0, 0), 0, 0);
        for i in 1..10u32 {
            assert!(!live.observe((i as i32 * 40, 0), i * TICK, i * TICK - 5), "evento recente do hook");
        }
    }

    #[test]
    fn contador_de_ms_do_windows_que_da_a_volta_nao_engana() {
        let mut live = Liveness::default();
        let start = u32::MAX - 100;
        live.observe((0, 0), start, start);
        assert!(!live.observe((10, 0), start.wrapping_add(TICK), start.wrapping_add(TICK) - 1));
    }

    #[test]
    fn processo_do_job_morre_quando_o_job_fecha() {
        let job = KillOnCloseJob::new().expect("job");
        let mut child = std::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("ping");
        job.assign(&child).expect("assign");
        drop(job);
        for _ in 0..50 {
            if child.try_wait().expect("try_wait").is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = child.kill();
        panic!("o filho sobreviveu ao fechamento do job");
    }
}
