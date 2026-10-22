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

/// Arrumação nova das telas da casa, feita no editor.
#[derive(Debug)]
pub struct HomeArrangement {
    /// Layout salvo deslocado para a principal ficar em (0,0), como no Windows.
    pub layout: ScreenLayout,
    /// Posições (id nativo, x, y) a aplicar no Windows; vazio se ele já está assim.
    pub positions: Vec<(String, i32, i32)>,
}

/// O editor moveu as telas da casa? Só com todas conhecidas e encostadas: o Windows não aceita telas soltas.
pub fn home_arrangement(saved: &ScreenLayout, home: &str, current: &[MonitorInfo]) -> Option<HomeArrangement> {
    let wanted = |info: &MonitorInfo| saved.monitores.iter().find(|m| m.id == global_id(home, &info.id));
    let anchor = wanted(current.iter().find(|info| (info.x, info.y) == (0, 0))?)?;
    let (dx, dy) = (anchor.x, anchor.y);
    let targets: Vec<(String, i32, i32)> = current
        .iter()
        .map(|info| wanted(info).map(|m| (info.id.clone(), m.x - dx, m.y - dy)))
        .collect::<Option<_>>()?;
    let unchanged = targets.iter().zip(current).all(|((_, x, y), info)| (*x, *y) == (info.x, info.y));
    if unchanged && (dx, dy) == (0, 0) {
        return None;
    }
    let homes: Vec<&Monitor> = saved.monitores.iter().filter(|m| m.machine == home).collect();
    if !connected(&homes) {
        return None;
    }
    let shifted = saved.monitores.iter().map(|m| Monitor { x: m.x - dx, y: m.y - dy, ..m.clone() });
    Some(HomeArrangement {
        layout: ScreenLayout { monitores: shifted.collect() },
        positions: if unchanged { Vec::new() } else { targets },
    })
}

/// Encostam quando dividem um trecho de borda (só o canto não basta): é por ali que o cursor passa.
fn touching(a: &Monitor, b: &Monitor) -> bool {
    let rows = a.y < b.y + b.height && b.y < a.y + a.height;
    let cols = a.x < b.x + b.width && b.x < a.x + a.width;
    ((a.x + a.width == b.x || b.x + b.width == a.x) && rows)
        || ((a.y + a.height == b.y || b.y + b.height == a.y) && cols)
}

fn connected(monitors: &[&Monitor]) -> bool {
    let mut reached = monitors.iter().take(1).copied().collect::<Vec<_>>();
    let mut i = 0;
    while let Some(m) = reached.get(i).copied() {
        for o in monitors {
            if !reached.iter().any(|r| r.id == o.id) && touching(m, o) {
                reached.push(o);
            }
        }
        i += 1;
    }
    reached.len() == monitors.len()
}

/// Monitores desconhecidos da máquina entram à direita do layout, preservando o arranjo entre eles.
/// Os conhecidos ficam onde o usuário pôs, com o tamanho da resolução atual (e o "tamanho na mesa" dele).
pub fn merge_monitors(layout: &ScreenLayout, machine: &str, monitors: &[MonitorInfo]) -> ScreenLayout {
    let current =
        |m: &Monitor| monitors.iter().find(|info| m.machine == machine && global_id(machine, &info.id) == m.id);
    let resized = layout.monitores.iter().map(|m| match current(m) {
        Some(info) => Monitor {
            width: (info.width as f64 * m.scale_fator).round() as i32,
            height: (info.height as f64 * m.scale_fator).round() as i32,
            ..m.clone()
        },
        None => m.clone(),
    });
    let layout = ScreenLayout { monitores: resized.collect() };
    let known = |info: &&MonitorInfo| layout.monitores.iter().any(|m| m.id == global_id(machine, &info.id));
    let new: Vec<&MonitorInfo> = monitors.iter().filter(|info| !known(info)).collect();
    if new.is_empty() {
        return layout;
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

/// Só a casa e as telas que cada cliente conectado informou agora: máquina ausente ou tela que sumiu
/// (cabo trocado, monitor desligado) vira parede, não destino. O layout guarda a posição para quando voltar.
pub fn connected_view(layout: &ScreenLayout, home: &str, connected: &[(String, Vec<MonitorInfo>)]) -> ScreenLayout {
    let present = |m: &Monitor| {
        connected
            .iter()
            .any(|(machine, monitors)| *machine == m.machine && monitors.iter().any(|i| i.id == native_id(&m.id)))
    };
    ScreenLayout { monitores: layout.monitores.iter().filter(|m| m.machine == home || present(m)).cloned().collect() }
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
        let win2 = vec![("win2".to_string(), vec![info("1", 0, 1920, 1080, 1.0)])];
        assert_eq!(connected_view(&layout, "home", &win2).monitores.len(), 2);
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

    fn at(layout: &ScreenLayout, id: &str) -> (i32, i32) {
        let m = layout.monitores.iter().find(|m| m.id == id).expect("monitor");
        (m.x, m.y)
    }

    fn mesa() -> (ScreenLayout, Vec<MonitorInfo>) {
        // Ultrawide principal em (0,0), notebook à direita; mac acima do ultrawide.
        let homes = vec![info("5", 0, 3440, 1440, 1.0), info("1", 3440, 2560, 1600, 1.0)];
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "pc", &homes);
        let mut mac = layout.monitores[0].clone();
        (mac.id, mac.machine, mac.y) = ("mac:3".into(), "mac".into(), -1440);
        (ScreenLayout { monitores: layout.monitores.iter().cloned().chain([mac]).collect() }, homes)
    }

    #[test]
    fn arrumacao_igual_a_do_windows_nao_muda_nada() {
        let (saved, homes) = mesa();
        assert!(home_arrangement(&saved, "pc", &homes).is_none());
    }

    #[test]
    fn notebook_movido_para_a_esquerda_rearruma_o_windows_com_a_principal_em_zero() {
        let (mut saved, homes) = mesa();
        // No editor: notebook à esquerda do ultrawide, que foi para x=2560 (o editor não ancora em zero).
        for m in &mut saved.monitores {
            match m.id.as_str() {
                "pc:1" => (m.x, m.y) = (0, 0),
                _ => m.x += 2560,
            }
        }
        let plan = home_arrangement(&saved, "pc", &homes).expect("muda");
        assert_eq!(plan.positions, vec![("5".to_string(), 0, 0), ("1".to_string(), -2560, 0)]);
        assert_eq!(at(&plan.layout, "pc:5"), (0, 0), "layout acompanha: casa nas coordenadas reais");
        assert_eq!(at(&plan.layout, "pc:1"), (-2560, 0));
        assert_eq!(at(&plan.layout, "mac:3"), (0, -1440), "outras máquinas mantêm a posição relativa");
    }

    #[test]
    fn casa_inteira_deslocada_so_ajusta_o_layout() {
        let (mut saved, homes) = mesa();
        for m in saved.monitores.iter_mut().filter(|m| m.machine == "pc") {
            m.x += 100;
        }
        let plan = home_arrangement(&saved, "pc", &homes).expect("muda");
        assert!(plan.positions.is_empty(), "o Windows já está assim");
        assert_eq!(at(&plan.layout, "mac:3"), (-100, -1440));
    }

    #[test]
    fn telas_soltas_ou_desconhecidas_nao_mexem_no_windows() {
        let (mut saved, homes) = mesa();
        saved.monitores.iter_mut().find(|m| m.id == "pc:1").expect("notebook").x = 4000;
        assert!(home_arrangement(&saved, "pc", &homes).is_none(), "vão entre as telas");
        let (saved, mut homes) = mesa();
        homes.push(info("9", 6000, 1920, 1080, 1.0));
        assert!(home_arrangement(&saved, "pc", &homes).is_none(), "tela que o layout não conhece");
    }

    #[test]
    fn tela_que_o_cliente_nao_informa_mais_vira_parede() {
        // Cabo trocado de HDMI para DP: o Mac renumera a tela; a antiga fica no layout, mas não é destino.
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        let layout = merge_monitors(&layout, "mac", &[info("3", 0, 2560, 1440, 1.0)]);
        let layout = merge_monitors(&layout, "mac", &[info("2", 0, 2560, 1440, 1.0)]);
        let now = vec![("mac".to_string(), vec![info("2", 0, 2560, 1440, 1.0)])];
        let ids: Vec<String> = connected_view(&layout, "home", &now).monitores.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["home:a", "mac:2"]);
    }

    #[test]
    fn resolucao_nova_de_tela_conhecida_atualiza_o_tamanho_e_mantem_posicao_e_tamanho_na_mesa() {
        let layout = ensure_home(&ScreenLayout { monitores: vec![] }, "home", &[info("a", 0, 1920, 1080, 1.0)]);
        let mut layout = merge_monitors(&layout, "mac", &[info("2", 0, 2560, 1440, 1.0)]);
        // Usuário ajustou o "tamanho na mesa" e a posição no editor.
        let mac = layout.monitores.iter_mut().find(|m| m.id == "mac:2").expect("mac");
        (mac.x, mac.y, mac.width, mac.height, mac.scale_fator) = (0, -2000, 5120, 2880, 2.0);
        let updated = merge_monitors(&layout, "mac", &[info("2", 0, 3440, 1440, 1.0)]);
        let mac = updated.monitores.iter().find(|m| m.id == "mac:2").expect("mac");
        assert_eq!((mac.x, mac.y, mac.width, mac.height, mac.scale_fator), (0, -2000, 6880, 2880, 2.0));
        assert_eq!(updated.monitores.len(), 2);
    }
}
