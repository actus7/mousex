//! Arrumação das telas no Windows: a mesma que o usuário fez no editor do MouseX.
use std::{io, mem, ptr};

use windows_sys::Win32::{
    Foundation::POINTL,
    Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_NORESET, CDS_UPDATEREGISTRY, DEVMODEW,
        DISP_CHANGE_SUCCESSFUL, DM_POSITION, ENUM_CURRENT_SETTINGS,
    },
};

/// Move cada tela (`\\.\DISPLAYn`, x, y) e aplica tudo de uma vez. A principal precisa ficar em (0,0).
// ponytail: se uma tela falhar no meio, as anteriores ficam gravadas e valem na próxima troca de modo;
// desfazer exigiria guardar e regravar o arranjo antigo.
pub fn set_monitor_positions(positions: &[(String, i32, i32)]) -> io::Result<()> {
    for (device, x, y) in positions {
        let name: Vec<u16> = device.encode_utf16().chain([0]).collect();
        // SAFETY: nome terminado em zero; DEVMODEW local zerado com dmSize correto; ponteiros nulos aceitos pela API.
        let result = unsafe {
            let mut mode: DEVMODEW = mem::zeroed();
            mode.dmSize = mem::size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) == 0 {
                return Err(io::Error::other(format!("{device}: current display mode unavailable")));
            }
            mode.dmFields = DM_POSITION;
            mode.Anonymous1.Anonymous2.dmPosition = POINTL { x: *x, y: *y };
            ChangeDisplaySettingsExW(
                name.as_ptr(),
                &mode,
                ptr::null_mut(),
                CDS_UPDATEREGISTRY | CDS_NORESET,
                ptr::null(),
            )
        };
        if result != DISP_CHANGE_SUCCESSFUL {
            return Err(io::Error::other(format!("{device}: ChangeDisplaySettingsEx returned {result}")));
        }
    }
    // SAFETY: sem dispositivo nem modo, a API aplica as mudanças pendentes gravadas acima.
    let result = unsafe { ChangeDisplaySettingsExW(ptr::null(), ptr::null(), ptr::null_mut(), 0, ptr::null()) };
    if result != DISP_CHANGE_SUCCESSFUL {
        return Err(io::Error::other(format!("applying display arrangement returned {result}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "usa as telas reais; rodar à mão com --ignored"]
    fn reaplicar_a_arrumacao_atual_e_aceito() {
        crate::init_dpi_awareness();
        let current: Vec<(String, i32, i32)> =
            crate::monitors().expect("telas").into_iter().map(|m| (m.name, m.bounds.left, m.bounds.top)).collect();
        super::set_monitor_positions(&current).expect("Windows aceita o próprio arranjo");
    }
}
