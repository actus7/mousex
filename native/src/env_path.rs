//! PATH do sistema: a pasta de instalação entra nele para `mousex` funcionar em qualquer terminal.
use std::{io, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::WIN32_ERROR,
    System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
        KEY_SET_VALUE, REG_EXPAND_SZ,
    },
    UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE},
};

const ENVIRONMENT_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const BROADCAST_TIMEOUT_MS: u32 = 5000;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn check(code: WIN32_ERROR) -> io::Result<()> {
    match code {
        0 => Ok(()),
        _ => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// PATH com `dir` incluído (ou removido); `None` se já está como pedido.
pub fn edit_path(path: &str, dir: &str, present: bool) -> Option<String> {
    // Entrada com aspas ("C:\Program Files\MouseX") também vale no PATH.
    let same = |entry: &str| {
        entry.trim().trim_matches('"').trim_end_matches('\\').eq_ignore_ascii_case(dir.trim_end_matches('\\'))
    };
    let entries: Vec<&str> = path.split(';').filter(|e| !e.trim().is_empty()).collect();
    match (present, entries.iter().any(|e| same(e))) {
        (true, false) => Some(entries.into_iter().chain([dir]).collect::<Vec<_>>().join(";")),
        (false, true) => Some(entries.into_iter().filter(|e| !same(e)).collect::<Vec<_>>().join(";")),
        _ => None,
    }
}

/// Põe ou tira `dir` do PATH do sistema (pede administrador) e avisa o Explorer: terminais novos já enxergam.
/// Lê o valor cru (`%SystemRoot%` continua sem expandir) e grava de volta como REG_EXPAND_SZ.
pub fn set_in_system_path(dir: &Path, present: bool) -> io::Result<()> {
    let (key_name, value) = (wide(ENVIRONMENT_KEY), wide("Path"));
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: nome terminado em NUL; `key` recebe o handle e é fechado abaixo.
    check(unsafe {
        RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_name.as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut key)
    })?;
    let result = update(key, &value, dir, present);
    // SAFETY: handle aberto acima, fechado uma vez.
    unsafe { RegCloseKey(key) };
    result
}

fn update(key: HKEY, value: &[u16], dir: &Path, present: bool) -> io::Result<()> {
    let mut size = 0u32;
    // SAFETY: primeira chamada só mede o tamanho em bytes.
    check(unsafe { RegQueryValueExW(key, value.as_ptr(), ptr::null(), ptr::null_mut(), ptr::null_mut(), &mut size) })?;
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    // SAFETY: buffer com pelo menos `size` bytes; o registro escreve até `size`.
    check(unsafe {
        RegQueryValueExW(key, value.as_ptr(), ptr::null(), ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size)
    })?;
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let current = String::from_utf16_lossy(&buffer[..len]);
    let Some(path) = edit_path(&current, &dir.display().to_string(), present) else {
        return Ok(());
    };
    let data = wide(&path);
    // SAFETY: `data` UTF-16 terminado em NUL, tamanho em bytes incluindo o terminador.
    check(unsafe {
        RegSetValueExW(key, value.as_ptr(), 0, REG_EXPAND_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
    })?;
    let environment = wide("Environment");
    // SAFETY: lParam aponta para string viva durante a chamada; timeout evita travar em janela pendurada.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            BROADCAST_TIMEOUT_MS,
            ptr::null_mut(),
        )
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::edit_path;

    const DIR: &str = r"C:\Program Files\MouseX";

    #[test]
    fn inclui_uma_vez_e_preserva_variaveis() {
        let path = r"%SystemRoot%\system32;C:\Tools;";
        assert_eq!(
            edit_path(path, DIR, true).as_deref(),
            Some(r"%SystemRoot%\system32;C:\Tools;C:\Program Files\MouseX")
        );
        assert_eq!(edit_path(r"C:\Tools;c:\program files\mousex\", DIR, true), None, "já está");
    }

    #[test]
    fn remove_so_a_pasta_do_mousex() {
        let path = r"C:\Tools;C:\Program Files\MouseX;%SystemRoot%";
        assert_eq!(edit_path(path, DIR, false).as_deref(), Some(r"C:\Tools;%SystemRoot%"));
        assert_eq!(edit_path(r"C:\Tools", DIR, false), None);
        assert_eq!(edit_path(r#"C:\Tools;"C:\Program Files\MouseX""#, DIR, false).as_deref(), Some(r"C:\Tools"));
        assert_eq!(edit_path(r#""C:\Program Files\MouseX""#, DIR, true), None, "com aspas já está");
    }
}
