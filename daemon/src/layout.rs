//! Layout persistido: ids globais "máquina:monitor", monitores novos entram à direita.
use mousex_core::{Monitor, ScreenLayout, proto::MonitorInfo};

fn global_id(machine: &str, id: &str) -> String {
    format!("{machine}:{id}")
}

/// Id nativo (o que a máquina dona reportou) a partir do id global.
pub fn native_id(global: &str) -> &str {
    global.split_once(':').map_or(global, |(_, id)| id)
}

fn monitor(machine: &str, info: &MonitorInfo, x: i32, y: i32) -> Monitor {
    Monitor {
        id: global_id(machine, &info.id),
        machine: machine.into(),
        x,
        y,
        width: (info.width as f64 * info.scale).round() as i32,
        height: (info.height as f64 * info.scale).round() as i32,
        scale_fator: info.scale,
    }
}

/// Monitores da casa sempre nas coordenadas reais: o hook compara o cursor físico com elas.
pub fn ensure_home(layout: &ScreenLayout, home: &str, monitors: &[MonitorInfo]) -> ScreenLayout {
    let others = layout.monitores.iter().filter(|m| m.machine != home).cloned();
    let homes = monitors.iter().map(|info| monitor(home, info, info.x, info.y));
    ScreenLayout { monitores: homes.chain(others).collect() }
}

/// Acrescenta só monitores desconhecidos da máquina, à direita do layout, preservando o arranjo entre eles.
pub fn merge_monitors(layout: &ScreenLayout, machine: &str, monitors: &[MonitorInfo]) -> ScreenLayout {
    let known = |info: &&MonitorInfo| layout.monitores.iter().any(|m| m.id == global_id(machine, &info.id));
    let new: Vec<&MonitorInfo> = monitors.iter().filter(|info| !known(info)).collect();
    if new.is_empty() {
        return layout.clone();
    }
    let right = layout.monitores.iter().map(|m| m.x + m.width).max().unwrap_or(0);
    let top = layout.monitores.iter().map(|m| m.y).min().unwrap_or(0);
    let min_x = new.iter().map(|i| i.x).min().unwrap_or(0);
    // O monitor mais à esquerda encosta no topo do layout: é por ele que o cursor entra vindo da esquerda.
    let min_y = new.iter().filter(|i| i.x == min_x).map(|i| i.y).min().unwrap_or(0);
    let placed = new.iter().map(|info| {
        let x = right + ((info.x - min_x) as f64 * info.scale).round() as i32;
        let y = top + ((info.y - min_y) as f64 * info.scale).round() as i32;
        monitor(machine, info, x, y)
    });
    ScreenLayout { monitores: layout.monitores.iter().cloned().chain(placed).collect() }
}

/// Acrescenta a máquina só se o resultado for um layout válido: layout inválido desliga todas as transições.
pub fn add_machine(
    layout: &ScreenLayout,
    machine: &str,
    monitors: &[MonitorInfo],
) -> Result<ScreenLayout, mousex_core::LayoutError> {
    let merged = merge_monitors(layout, machine, monitors);
    merged.validate()?;
    Ok(merged)
}

/// Layout efetivo: o salvo, com a casa nas coordenadas reais e todo cliente conectado alcançável.
/// Se a casa nova colidir com o salvo, recomeça só com a casa; clientes conhecidos mantêm posição quando possível.
pub fn rebuild(
    saved: &ScreenLayout,
    home: &str,
    home_monitors: &[MonitorInfo],
    clients: &[(String, Vec<MonitorInfo>)],
) -> ScreenLayout {
    let with_home = ensure_home(saved, home, home_monitors);
    let base = if with_home.validate().is_ok() {
        with_home
    } else {
        ensure_home(&ScreenLayout { monitores: Vec::new() }, home, home_monitors)
    };
    clients.iter().fold(base, |layout, (machine, monitors)| add_machine(&layout, machine, monitors).unwrap_or(layout))
}

/// Só casa e máquinas conectadas: máquina ausente vira parede, não destino.
pub fn connected_view(layout: &ScreenLayout, home: &str, connected: &[String]) -> ScreenLayout {
    ScreenLayout {
        monitores: layout
            .monitores
            .iter()
            .filter(|m| m.machine == home || connected.contains(&m.machine))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, x: i32, width: i32, height: i32, scale: f64) -> MonitorInfo {
        MonitorInfo { id: id.into(), x, y: 0, width, height, scale }
    }

    #[test]
    fn hello_adiciona_monitores_ao_layout() {
        let layout =
            ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info(r"\.\DISPLAY1", 0, 1920, 1080, 1.0)]);
        let layout = merge_monitors(&layout, "win2", &[info(r"\.\DISPLAY1", 0, 1920, 1080, 1.0)]);
        let added = layout.monitores.iter().find(|m| m.id == r"win2:\.\DISPLAY1").expect("win2");
        assert_eq!((added.x, added.y, added.machine.as_str()), (1920, 0, "win2"));
        let again = merge_monitors(&layout, "win2", &[info(r"\.\DISPLAY1", 0, 1920, 1080, 1.0)]);
        assert_eq!(again.monitores.len(), 2);
        let with_mac = merge_monitors(&again, "mac", &[info("1", 0, 1440, 900, 2.0)]);
        let mac = with_mac.monitores.iter().find(|m| m.machine == "mac").expect("mac");
        assert_eq!((mac.x, mac.width, mac.height, mac.scale_fator), (3840, 2880, 1800, 2.0));
        with_mac.validate().expect("layout válido");
    }

    #[test]
    fn varios_monitores_do_cliente_mantem_arranjo() {
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        let layout =
            merge_monitors(&layout, "win2", &[info("1", 0, 1920, 1080, 1.0), info("2", -1280, 1280, 1024, 1.0)]);
        let x = |id: &str| layout.monitores.iter().find(|m| m.id == id).expect("monitor").x;
        assert_eq!((x("win2:2"), x("win2:1")), (1920, 3200));
        layout.validate().expect("layout válido");
    }

    #[test]
    fn home_sempre_reflete_coordenadas_reais() {
        let stale = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1280, 720, 1.0)]);
        let fresh = ensure_home(&stale, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        assert_eq!(fresh.monitores.len(), 1);
        assert_eq!(fresh.monitores[0].width, 1920);
    }

    #[test]
    fn visao_conectada_esconde_maquinas_ausentes() {
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        let layout = merge_monitors(&layout, "win2", &[info("1", 0, 1920, 1080, 1.0)]);
        assert_eq!(connected_view(&layout, "home", &[]).monitores.len(), 1);
        assert_eq!(connected_view(&layout, "home", &["win2".to_string()]).monitores.len(), 2);
        assert_eq!(native_id("win2:1"), "1");
    }

    #[test]
    fn maquina_que_quebraria_o_layout_e_recusada() {
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        // Escalas diferentes na mesma máquina não podem gerar sobreposição aceita.
        let broken = [info("1", 0, 1470, 956, 2.0), info("3", 1470, 2560, 1440, 1.0)];
        assert!(add_machine(&layout, "mac", &broken).is_err());
        let ok = add_machine(&layout, "mac", &[info("1", 0, 1470, 956, 1.0), info("3", 1470, 2560, 1440, 1.0)])
            .expect("válido");
        assert_eq!(ok.monitores.len(), 3);
    }

    #[test]
    fn maquina_nova_encosta_pelo_monitor_mais_a_esquerda() {
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        // Mac real: notebook (0,1215) à esquerda e abaixo do externo (1470,0).
        let mut notebook = info("1", 0, 1470, 956, 1.0);
        notebook.y = 1215;
        let layout = add_machine(&layout, "mac", &[notebook, info("3", 1470, 2560, 1440, 1.0)]).expect("válido");
        let pos = |id: &str| {
            let m = layout.monitores.iter().find(|m| m.id == id).expect("monitor");
            (m.x, m.y)
        };
        assert_eq!(pos("mac:1"), (1920, 0), "notebook encosta na casa");
        assert_eq!(pos("mac:3"), (3390, -1215), "externo mantém a posição relativa");
    }

    #[test]
    fn reconstruir_mantem_clientes_conectados() {
        // Layout salvo antigo (sem o mac) e casa que mudou: o mac conectado continua alcançável.
        let saved = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1280, 720, 1.0)]);
        let clients = vec![("mac".to_string(), vec![info("1", 0, 1470, 956, 1.0)])];
        let rebuilt = rebuild(&saved, "home", &[info("a", 0, 1920, 1080, 1.0)], &clients);
        assert!(rebuilt.monitores.iter().any(|m| m.id == "mac:1"));
        assert!(rebuilt.monitores.iter().any(|m| m.id == "home:a" && m.width == 1920));
        rebuilt.validate().expect("válido");
    }

    #[test]
    fn reconstruir_com_casa_sobrepondo_recomeca_e_mantem_clientes() {
        let mut saved = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        saved = add_machine(&saved, "mac", &[info("1", 0, 1470, 956, 1.0)]).expect("mac");
        // Monitor novo da casa ocupa exatamente onde o mac estava.
        let homes = [info("a", 0, 1920, 1080, 1.0), info("b", 1920, 1920, 1080, 1.0)];
        let clients = vec![("mac".to_string(), vec![info("1", 0, 1470, 956, 1.0)])];
        let rebuilt = rebuild(&saved, "home", &homes, &clients);
        rebuilt.validate().expect("válido");
        assert_eq!(rebuilt.monitores.len(), 3);
    }
}
