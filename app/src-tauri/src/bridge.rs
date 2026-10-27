// Editor de telas: lê e grava o layout do servidor no caminho passado pelo agente (`--layout <arquivo>`).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Monitor {
    id: String,
    machine: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_fator: f64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    monitores: Vec<Monitor>,
}

fn argument(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

/// Mesmo idioma do agente: `MOUSEX_LANG` (herdado dele) ou o do sistema; português ou inglês.
#[tauri::command]
pub fn portuguese() -> bool {
    std::env::var("MOUSEX_LANG")
        .ok()
        .or_else(sys_locale::get_locale)
        .is_some_and(|locale| locale.to_lowercase().starts_with("pt"))
}

fn t(pt: &str, en: &str) -> String {
    if portuguese() { pt } else { en }.to_string()
}

fn layout_path() -> Result<PathBuf, String> {
    argument("--layout").map(PathBuf::from).ok_or_else(|| {
        t("Abra o editor pelo ícone do MouseX na bandeja.", "Open the editor from the MouseX tray icon.")
    })
}

/// Máquina do servidor: as telas dela seguem as coordenadas reais do Windows e não podem ser movidas aqui.
#[tauri::command]
pub fn home_machine() -> Option<String> {
    argument("--home")
}

#[tauri::command]
pub fn load_layout() -> Result<Layout, String> {
    let bytes = std::fs::read(layout_path()?)
        .map_err(|e| format!("{} {e}", t("Não foi possível ler o layout:", "Could not read the layout:")))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{} {e}", t("Layout inválido:", "Invalid layout:")))
}

/// Ids globais das telas conectadas quando o agente abriu o editor; `None` fora do agente.
#[tauri::command]
pub fn connected_screens() -> Option<Vec<String>> {
    argument("--connected").map(|ids| ids.split(',').filter(|id| !id.is_empty()).map(String::from).collect())
}

/// Valida e grava de forma atômica; avisa o agente pela saída padrão para ele recarregar o layout.
#[tauri::command]
pub fn save_layout(layout: Layout) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for (i, a) in layout.monitores.iter().enumerate() {
        if a.id.trim().is_empty()
            || a.machine.trim().is_empty()
            || !ids.insert(&a.id)
            || a.width == 0
            || a.height == 0
            || a.width > i32::MAX as u32
            || a.height > i32::MAX as u32
            || !a.scale_fator.is_finite()
            || !(0.5..=4.0).contains(&a.scale_fator)
        {
            return Err(t("Tela inválida: confira dimensões e tamanho.", "Invalid screen: check its size and scale."));
        }
        for b in &layout.monitores[i + 1..] {
            if i64::from(a.x) < i64::from(b.x) + i64::from(b.width)
                && i64::from(a.x) + i64::from(a.width) > i64::from(b.x)
                && i64::from(a.y) < i64::from(b.y) + i64::from(b.height)
                && i64::from(a.y) + i64::from(a.height) > i64::from(b.y)
            {
                return Err(t("Telas sobrepostas.", "Screens overlap."));
            }
        }
    }
    let data = serde_json::to_vec_pretty(&layout).map_err(|e| e.to_string())?;
    crate::persistence::save(&layout_path()?, &data)
        .map_err(|e| format!("{} {e}", t("Não foi possível salvar:", "Could not save:")))?;
    // ponytail: sem agente ouvindo (stdout fechado) o println falharia; o arquivo já foi gravado.
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = writeln!(out, "saved").and_then(|_| out.flush());
    Ok(())
}

#[tauri::command]
pub fn close_editor(app: tauri::AppHandle) {
    app.exit(0);
}
