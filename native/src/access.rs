//! Acesso a arquivos do usuário a partir do agente elevado: leitura com os direitos do usuário comum,
//! pasta privada para o que chega de outras máquinas e espaço livre em disco.
use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE},
    Security::{
        Authorization::ConvertSidToStringSidW, CreateRestrictedToken, CreateWellKnownSid, GetTokenInformation,
        ImpersonateLoggedOnUser, RevertToSelf, TokenUser, WinBuiltinAdministratorsSid, DISABLE_MAX_PRIVILEGE,
        SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
        TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::GetDiskFreeSpaceExW,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

/// Fecha o handle ao sair do escopo.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle aberto por esta struct e fechado uma vez.
        unsafe { CloseHandle(self.0) };
    }
}

fn process_token(access: u32) -> io::Result<Handle> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: pseudo-handle do próprio processo; token devolvido é fechado por `Handle`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(token))
}

/// Roda `f` nesta thread com o token do processo sem privilégios e com Administradores só para negar:
/// o agente elevado abre só o que o próprio usuário abriria (um caminho plantado no clipboard por outro
/// programa não lê arquivos protegidos). Handles abertos dentro de `f` continuam valendo depois.
pub fn as_standard_user<T>(f: impl FnOnce() -> T) -> io::Result<T> {
    let token = process_token(TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_IMPERSONATE)?;
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    // SAFETY: buffer do tamanho máximo de SID; token restrito fechado por `Handle`; RevertToSelf sempre chamado.
    unsafe {
        if CreateWellKnownSid(WinBuiltinAdministratorsSid, ptr::null_mut(), sid.as_mut_ptr() as _, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let admins = SID_AND_ATTRIBUTES { Sid: sid.as_mut_ptr() as _, Attributes: 0 };
        let mut restricted: HANDLE = ptr::null_mut();
        if CreateRestrictedToken(
            token.0,
            DISABLE_MAX_PRIVILEGE,
            1,
            &admins,
            0,
            ptr::null(),
            0,
            ptr::null(),
            &mut restricted,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let restricted = Handle(restricted);
        if ImpersonateLoggedOnUser(restricted.0) == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = f();
        RevertToSelf();
        Ok(result)
    }
}

/// SID do usuário deste processo, em texto ("S-1-5-21-…").
fn current_user_sid() -> io::Result<String> {
    let token = process_token(TOKEN_QUERY)?;
    let mut buffer = vec![0u8; 256];
    let mut size = 0u32;
    // SAFETY: buffer com o tamanho informado; string do SID alocada pelo sistema e liberada com LocalFree.
    unsafe {
        if GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr() as _, buffer.len() as u32, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text: windows_sys::core::PWSTR = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text as _);
        Ok(sid)
    }
}

/// Pasta para o que chega de outras máquinas: Administradores e SYSTEM gravam, o usuário só lê
/// (nenhum programa comum planta ali uma junction para o agente elevado seguir). Falha se já existe.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let sid = current_user_sid()?;
    super::create_dir_with_sddl(path, &format!("D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;{sid})"))
}

/// Bytes livres para este usuário no disco de `path`.
pub fn free_space(path: &Path) -> io::Result<u64> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: caminho terminado em NUL; só o primeiro contador é pedido.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, ptr::null_mut(), ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codigo_roda_como_usuario_comum_e_volta() {
        let file = std::env::temp_dir().join(format!("mousex-acesso-{}", std::process::id()));
        std::fs::write(&file, "x").expect("arquivo");
        assert_eq!(as_standard_user(|| std::fs::read_to_string(&file).ok()).expect("restrito"), Some("x".into()));
        assert!(std::fs::read_to_string(&file).is_ok(), "token próprio de volta");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn pasta_privada_nasce_sem_heranca() {
        let dir = std::env::temp_dir().join(format!("mousex-privada-{}", std::process::id()));
        let _ = std::fs::remove_dir(&dir);
        create_private_dir(&dir).expect("criar");
        let acl = std::process::Command::new("icacls").arg(&dir).output().expect("icacls");
        assert!(!String::from_utf8_lossy(&acl.stdout).contains("(I)"));
        assert_eq!(create_private_dir(&dir).expect_err("já existe").kind(), std::io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn espaco_livre_do_disco() {
        assert!(free_space(&std::env::temp_dir()).expect("espaço") > 0);
    }
}
