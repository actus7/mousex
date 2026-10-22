//! Assistente de instalação: reabrir como administrador, esconder o console e mostrar mensagens.
use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_CANCELLED},
    System::Console::FreeConsole,
    UI::{
        Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
        WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, SW_HIDE,
        },
    },
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Abre `exe` com `arguments` pedindo elevação (UAC), com o console escondido.
/// `Ok(false)` se o usuário recusou o pedido de administrador.
pub fn run_elevated(exe: &Path, arguments: &str) -> io::Result<bool> {
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let (verb, parameters) = (wide("runas"), wide(arguments));
    // SAFETY: estrutura zerada e preenchida aqui; strings vivas durante a chamada; handle do processo fechado.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_CANCELLED as i32) { Ok(false) } else { Err(error) };
        }
        if !info.hProcess.is_null() {
            CloseHandle(info.hProcess);
        }
        Ok(true)
    }
}

/// Solta a janela de console (o assistente fala por caixas de diálogo).
pub fn hide_console() {
    // SAFETY: chamada sem ponteiros; sem console, não faz nada.
    unsafe { FreeConsole() };
}

/// Caixa de mensagem simples, em primeiro plano.
pub fn show_message(title: &str, text: &str, error: bool) {
    let (title, text) = (wide(title), wide(text));
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: strings UTF-16 terminadas em NUL, vivas durante a chamada.
    unsafe {
        MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | icon | MB_TOPMOST | MB_SETFOREGROUND)
    };
}
