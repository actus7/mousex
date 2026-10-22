//! Tela acordada enquanto o usuário usa outra máquina, e bloqueio junto com o servidor.
use std::io;
use windows_sys::Win32::{
    System::{
        Power::{SetThreadExecutionState, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED},
        Shutdown::LockWorkStation,
    },
    UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_MOVE},
};

/// Zera o contador de inatividade: tela ligada e sem bloqueio por tempo enquanto houver atividade em outra máquina.
/// O movimento de 0 px não mexe o cursor, mas conta como entrada (o bloqueio por inatividade olha a última entrada).
pub fn keep_awake() {
    // SAFETY: sem ponteiros; estrutura INPUT zerada e preenchida aqui.
    unsafe {
        SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        let mut input: INPUT = std::mem::zeroed();
        input.r#type = INPUT_MOUSE;
        input.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE;
        input.Anonymous.mi.dwExtraInfo = super::INPUT_MARKER;
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

/// Bloqueia a sessão (igual a Win+L).
pub fn lock_workstation() -> io::Result<()> {
    // SAFETY: chamada sem ponteiros.
    if unsafe { LockWorkStation() } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
