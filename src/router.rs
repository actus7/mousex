//! Cursor virtual do servidor: decide entrada, movimento, troca e volta entre máquinas.
use crate::{Direction, Monitor, Point, ScreenLayout, edge_router};

/// Maior deslocamento crível num único evento de mouse (px). Acima disso é artefato
/// (evento calculado antes do cursor ser estacionado) e seria um salto falso no remoto.
pub const MAX_EVENT_DELTA: i32 = 1000;

pub fn plausible_delta(dx: i32, dy: i32) -> bool {
    dx.abs() <= MAX_EVENT_DELTA && dy.abs() <= MAX_EVENT_DELTA
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Home,
    Remote { machine: String, monitor_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Começar a suprimir a entrada local e estacionar o cursor.
    Enter {
        machine: String,
        monitor_id: String,
        local_x: i32,
        local_y: i32,
    },
    Move {
        machine: String,
        monitor_id: String,
        local_x: i32,
        local_y: i32,
    },
    /// Remoto → outro remoto.
    Switch {
        from: String,
        to: String,
        monitor_id: String,
        local_x: i32,
        local_y: i32,
    },
    /// Parar de suprimir e posicionar o cursor local.
    ReturnHome {
        global_x: i32,
        global_y: i32,
    },
}

pub struct Router {
    layout: ScreenLayout,
    home: String,
    focus: Focus,
    /// Posição global do cursor virtual enquanto o foco é remoto.
    pos: Point,
    /// Cursor preso na tela atual (jogos, apps em tela cheia): bordas não trocam de máquina.
    locked: bool,
}

impl Router {
    pub fn new(layout: ScreenLayout, home_machine: &str) -> Self {
        Self { layout, home: home_machine.into(), focus: Focus::Home, pos: Point { x: 0.0, y: 0.0 }, locked: false }
    }

    pub fn set_layout(&mut self, layout: ScreenLayout) {
        self.layout = layout;
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Leva o cursor ao centro do primeiro monitor de `machine` (atalho de teclado).
    pub fn jump_to(&mut self, machine: &str) -> Action {
        if machine == self.home {
            return self.force_home();
        }
        let Some(target) = self.layout.monitores.iter().find(|m| m.machine == machine).cloned() else {
            return Action::None;
        };
        let b = target.bounds();
        let center = Point { x: (b.left + b.right) / 2.0, y: (b.top + b.bottom) / 2.0 };
        let previous = std::mem::replace(
            &mut self.focus,
            Focus::Remote { machine: machine.into(), monitor_id: target.id.clone() },
        );
        let Action::Move { local_x, local_y, .. } = self.move_within(&target, center) else { return Action::None };
        let monitor_id = target.id;
        match previous {
            Focus::Home => Action::Enter { machine: machine.into(), monitor_id, local_x, local_y },
            Focus::Remote { machine: from, .. } if from == machine => {
                Action::Move { machine: from, monitor_id, local_x, local_y }
            }
            Focus::Remote { machine: from, .. } => {
                Action::Switch { from, to: machine.into(), monitor_id, local_x, local_y }
            }
        }
    }

    /// Cursor local em casa: só a borda de um monitor da casa pode iniciar transição.
    pub fn on_home_cursor(&mut self, global_x: i32, global_y: i32) -> Action {
        if self.focus != Focus::Home || self.locked {
            return Action::None;
        }
        let (raw_x, raw_y) = (global_x as f64, global_y as f64);
        // Monitor da casa que contém o ponto; se o ponto veio além da tela (movimento rápido), o mais próximo.
        let homes = self.layout.monitores.iter().filter(|m| m.machine == self.home);
        let Some(source) = homes.min_by(|a, b| distance(a, raw_x, raw_y).total_cmp(&distance(b, raw_x, raw_y))) else {
            return Action::None;
        };
        let b = source.bounds();
        let (x, y) = (raw_x.clamp(b.left, b.right - 1.0), raw_y.clamp(b.top, b.bottom - 1.0));
        let candidates = [
            (raw_x >= b.right - 1.0, Direction::Right, Point { x: b.right, y }),
            (raw_x <= b.left, Direction::Left, Point { x: b.left, y }),
            (raw_y >= b.bottom - 1.0, Direction::Down, Point { x, y: b.bottom }),
            (raw_y <= b.top, Direction::Up, Point { x, y: b.top }),
        ];
        for (at_edge, direction, point) in candidates {
            if !at_edge {
                continue;
            }
            let Ok(Some(t)) = edge_router(&self.layout, point, direction) else { continue };
            if t.machine == self.home {
                continue;
            }
            self.pos = t.global;
            self.focus = Focus::Remote { machine: t.machine.clone(), monitor_id: t.monitor_id.clone() };
            let (local_x, local_y) = floor(t.local);
            return Action::Enter { machine: t.machine, monitor_id: t.monitor_id, local_x, local_y };
        }
        Action::None
    }

    /// Delta do mouse físico (pixels do servidor) aplicado em pixels lógicos do monitor-alvo.
    pub fn on_delta(&mut self, dx: i32, dy: i32) -> Action {
        let Focus::Remote { machine, monitor_id } = self.focus.clone() else { return Action::None };
        let Some(current) = self.layout.monitores.iter().find(|m| m.id == monitor_id).cloned() else {
            return self.go_home();
        };
        let next =
            Point { x: self.pos.x + dx as f64 * current.scale_fator, y: self.pos.y + dy as f64 * current.scale_fator };
        let b = current.bounds();
        if contains(&current, next.x, next.y) {
            return self.move_within(&current, next);
        }
        let clamped =
            Point { x: next.x.clamp(b.left, b.right - f64::EPSILON), y: next.y.clamp(b.top, b.bottom - f64::EPSILON) };
        // Eixo com maior ultrapassagem decide a borda; o outro fica preso dentro do monitor.
        let over_x = (next.x - clamped.x).abs();
        let over_y = (next.y - clamped.y).abs();
        let (direction, point) = if over_x >= over_y {
            if next.x >= b.right {
                (Direction::Right, Point { x: b.right, y: clamped.y })
            } else {
                (Direction::Left, Point { x: b.left, y: clamped.y })
            }
        } else if next.y >= b.bottom {
            (Direction::Down, Point { x: clamped.x, y: b.bottom })
        } else {
            (Direction::Up, Point { x: clamped.x, y: b.top })
        };
        match edge_router(&self.layout, point, direction) {
            // Preso: só passa para outro monitor da mesma máquina.
            Ok(Some(t)) if t.machine == self.home && !self.locked => {
                self.focus = Focus::Home;
                // Um pixel para dentro: o pixel da borda é o gatilho de entrada e faria o cursor quicar de volta.
                let inward = match direction {
                    Direction::Left => Point { x: t.global.x - 1.0, ..t.global },
                    Direction::Right => Point { x: t.global.x + 1.0, ..t.global },
                    Direction::Up => Point { y: t.global.y - 1.0, ..t.global },
                    Direction::Down => Point { y: t.global.y + 1.0, ..t.global },
                };
                let landing =
                    self.layout.monitores.iter().any(|m| m.machine == self.home && contains(m, inward.x, inward.y));
                let (global_x, global_y) = floor(if landing { inward } else { t.global });
                Action::ReturnHome { global_x, global_y }
            }
            Ok(Some(t)) if !self.locked || t.machine == machine => {
                self.pos = t.global;
                self.focus = Focus::Remote { machine: t.machine.clone(), monitor_id: t.monitor_id.clone() };
                let (local_x, local_y) = floor(t.local);
                if t.machine == machine {
                    Action::Move { machine, monitor_id: t.monitor_id, local_x, local_y }
                } else {
                    Action::Switch { from: machine, to: t.machine, monitor_id: t.monitor_id, local_x, local_y }
                }
            }
            _ => self.move_within(&current, next),
        }
    }

    pub fn on_disconnect(&mut self, machine: &str) -> Action {
        match &self.focus {
            Focus::Remote { machine: focused, .. } if focused == machine => self.go_home(),
            _ => Action::None,
        }
    }

    pub fn force_home(&mut self) -> Action {
        if self.focus == Focus::Home { Action::None } else { self.go_home() }
    }

    /// Clampa em pixels lógicos inteiros do monitor e emite Move.
    fn move_within(&mut self, monitor: &Monitor, global: Point) -> Action {
        let Focus::Remote { machine, monitor_id } = self.focus.clone() else { return Action::None };
        let max_x = (monitor.width as f64 / monitor.scale_fator).floor() - 1.0;
        let max_y = (monitor.height as f64 / monitor.scale_fator).floor() - 1.0;
        let local = monitor.global_to_local(global);
        let local = Point { x: local.x.clamp(0.0, max_x.max(0.0)), y: local.y.clamp(0.0, max_y.max(0.0)) };
        self.pos = monitor.local_to_global(local);
        let (local_x, local_y) = floor(local);
        Action::Move { machine, monitor_id, local_x, local_y }
    }

    /// Volta ao centro do primeiro monitor da casa.
    fn go_home(&mut self) -> Action {
        self.focus = Focus::Home;
        let center = self
            .layout
            .monitores
            .iter()
            .find(|m| m.machine == self.home)
            .map(|m| {
                let b = m.bounds();
                Point { x: (b.left + b.right) / 2.0, y: (b.top + b.bottom) / 2.0 }
            })
            .unwrap_or(Point { x: 0.0, y: 0.0 });
        let (global_x, global_y) = floor(center);
        Action::ReturnHome { global_x, global_y }
    }
}

/// Máquinas da esquerda para a direita na mesa (numeração dos atalhos e do menu). Usar o layout completo,
/// não só o das conectadas: o número de cada máquina não muda quando outra desconecta.
pub fn machines_in_order(layout: &ScreenLayout) -> Vec<String> {
    let mut firsts: Vec<(i32, i32, &str)> = Vec::new();
    for m in &layout.monitores {
        match firsts.iter_mut().find(|(_, _, name)| *name == m.machine) {
            Some(entry) => {
                if (m.x, m.y) < (entry.0, entry.1) {
                    (entry.0, entry.1) = (m.x, m.y)
                }
            }
            None => firsts.push((m.x, m.y, &m.machine)),
        }
    }
    firsts.sort();
    firsts.into_iter().map(|(_, _, name)| name.to_string()).collect()
}

fn contains(m: &Monitor, x: f64, y: f64) -> bool {
    let b = m.bounds();
    x >= b.left && x < b.right && y >= b.top && y < b.bottom
}

/// Distância do ponto aos bounds do monitor (0 dentro).
fn distance(m: &Monitor, x: f64, y: f64) -> f64 {
    let b = m.bounds();
    let dx = (b.left - x).max(0.0).max(x - (b.right - 1.0));
    let dy = (b.top - y).max(0.0).max(y - (b.bottom - 1.0));
    dx.hypot(dy)
}

fn floor(p: Point) -> (i32, i32) {
    (p.x.floor() as i32, p.y.floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Monitor;

    fn monitor(id: &str, machine: &str, x: i32, width: i32, height: i32, scale: f64) -> Monitor {
        Monitor { id: id.into(), machine: machine.into(), x, y: 0, width, height, scale_fator: scale }
    }

    fn router() -> Router {
        let layout = ScreenLayout {
            monitores: vec![
                monitor("h", "home", 0, 1920, 1080, 1.0),
                monitor("w", "win2", 1920, 1920, 1080, 1.5),
                monitor("m", "mac", -1440, 1440, 900, 2.0),
            ],
        };
        Router::new(layout, "home")
    }

    fn entered_win2() -> Router {
        let mut r = router();
        assert!(matches!(r.on_home_cursor(1919, 500), Action::Enter { .. }));
        r
    }

    #[test]
    fn maquinas_numeradas_da_esquerda_para_a_direita() {
        assert_eq!(machines_in_order(&router().layout), ["mac", "home", "win2"]);
    }

    #[test]
    fn atalho_pula_para_o_centro_da_maquina() {
        let mut r = router();
        assert_eq!(
            r.jump_to("win2"),
            Action::Enter { machine: "win2".into(), monitor_id: "w".into(), local_x: 640, local_y: 360 }
        );
        assert_eq!(
            r.jump_to("mac"),
            Action::Switch {
                from: "win2".into(),
                to: "mac".into(),
                monitor_id: "m".into(),
                local_x: 360,
                local_y: 225
            }
        );
        assert_eq!(
            r.jump_to("mac"),
            Action::Move { machine: "mac".into(), monitor_id: "m".into(), local_x: 360, local_y: 225 }
        );
        assert_eq!(r.jump_to("home"), Action::ReturnHome { global_x: 960, global_y: 540 });
        assert_eq!(r.jump_to("home"), Action::None, "já em casa");
        assert_eq!(r.jump_to("desconhecida"), Action::None);
    }

    #[test]
    fn cursor_preso_nao_atravessa_bordas() {
        let mut r = router();
        r.set_locked(true);
        assert_eq!(r.on_home_cursor(1919, 500), Action::None);
        let mut r = entered_win2();
        r.set_locked(true);
        assert!(matches!(r.on_delta(-900, 0), Action::Move { ref machine, .. } if machine == "win2"));
        r.set_locked(false);
        assert!(matches!(r.on_delta(-900, 0), Action::ReturnHome { .. }));
    }

    #[test]
    fn cursor_preso_ainda_passa_entre_monitores_da_mesma_maquina() {
        let layout = ScreenLayout {
            monitores: vec![
                monitor("h", "home", 0, 1920, 1080, 1.0),
                monitor("w1", "win2", 1920, 1920, 1080, 1.0),
                monitor("w2", "win2", 3840, 1920, 1080, 1.0),
            ],
        };
        let mut r = Router::new(layout, "home");
        assert!(matches!(r.on_home_cursor(1919, 500), Action::Enter { .. }));
        r.set_locked(true);
        assert!(matches!(r.on_delta(2000, 0), Action::Move { ref monitor_id, .. } if monitor_id == "w2"));
        assert!(
            matches!(r.on_delta(2500, 0), Action::Move { ref monitor_id, .. } if monitor_id == "w2"),
            "sem vizinho: fica"
        );
    }

    #[test]
    fn borda_direita_entra_no_win2() {
        let mut r = router();
        assert_eq!(
            r.on_home_cursor(1919, 500),
            Action::Enter { machine: "win2".into(), monitor_id: "w".into(), local_x: 0, local_y: 333 }
        );
        assert_eq!(r.focus(), &Focus::Remote { machine: "win2".into(), monitor_id: "w".into() });
    }

    #[test]
    fn meio_da_tela_nao_faz_nada() {
        assert_eq!(router().on_home_cursor(900, 500), Action::None);
    }

    #[test]
    fn delta_move_no_remoto() {
        let mut r = entered_win2();
        assert_eq!(
            r.on_delta(10, -3),
            Action::Move { machine: "win2".into(), monitor_id: "w".into(), local_x: 10, local_y: 330 }
        );
    }

    #[test]
    fn delta_volta_para_casa_pela_esquerda() {
        let mut r = entered_win2();
        r.on_delta(5, 0);
        assert_eq!(r.on_delta(-6, 0), Action::ReturnHome { global_x: 1918, global_y: 500 });
        assert_eq!(r.focus(), &Focus::Home);
    }

    #[test]
    fn delta_grande_atravessa_borda() {
        let mut r = entered_win2();
        assert!(matches!(r.on_delta(-5000, 0), Action::ReturnHome { global_x: 1918, .. }));
    }

    #[test]
    fn escala_mista_e_coordenada_negativa() {
        let mut r = router();
        match r.on_home_cursor(0, 899) {
            Action::Enter { machine, local_x, local_y, .. } => {
                assert_eq!(machine, "mac");
                assert!((0..720).contains(&local_x), "x={local_x}");
                assert!((0..450).contains(&local_y), "y={local_y}");
            }
            other => panic!("esperava Enter, veio {other:?}"),
        }
        match r.on_delta(-100, 0) {
            Action::Move { local_x, .. } => assert!((0..720).contains(&local_x)),
            other => panic!("esperava Move, veio {other:?}"),
        }
    }

    #[test]
    fn sem_vizinho_clampa() {
        let mut r = entered_win2();
        assert!(matches!(r.on_delta(10_000, 0), Action::Move { local_x: 1279, .. }));
        assert!(matches!(r.on_delta(0, 10_000), Action::Move { local_y: 719, .. }));
    }

    #[test]
    fn queda_com_cursor_remoto_volta_para_casa() {
        let mut r = entered_win2();
        assert_eq!(r.on_disconnect("mac"), Action::None);
        assert!(matches!(r.on_disconnect("win2"), Action::ReturnHome { .. }));
        assert_eq!(r.focus(), &Focus::Home);
        assert_eq!(r.on_disconnect("win2"), Action::None);
    }

    #[test]
    fn hotkey_de_emergencia() {
        let mut r = entered_win2();
        assert!(matches!(r.force_home(), Action::ReturnHome { .. }));
        assert_eq!(r.force_home(), Action::None);
    }

    #[test]
    fn layout_sem_o_monitor_focado_volta_para_casa() {
        let mut r = entered_win2();
        r.set_layout(ScreenLayout { monitores: vec![monitor("h", "home", 0, 1920, 1080, 1.0)] });
        assert!(matches!(r.on_delta(1, 0), Action::ReturnHome { .. }));
    }

    #[test]
    fn remoto_para_remoto_troca() {
        let layout = ScreenLayout {
            monitores: vec![
                monitor("h", "home", 0, 1920, 1080, 1.0),
                monitor("w", "win2", 1920, 1920, 1080, 1.0),
                monitor("m", "mac", 3840, 1920, 1080, 1.0),
            ],
        };
        let mut r = Router::new(layout, "home");
        r.on_home_cursor(1919, 10);
        assert_eq!(
            r.on_delta(1920, 0),
            Action::Switch { from: "win2".into(), to: "mac".into(), monitor_id: "m".into(), local_x: 0, local_y: 10 }
        );
    }

    #[test]
    fn volta_nao_quica_para_o_remoto() {
        let mut r = entered_win2();
        let Action::ReturnHome { global_x, global_y } = r.on_delta(-10, 0) else { panic!("esperava ReturnHome") };
        // Movimento só vertical logo após voltar não pode reentrar no remoto.
        assert_eq!(r.on_home_cursor(global_x, global_y + 1), Action::None);
    }

    #[test]
    fn ponto_alem_da_borda_da_casa_tambem_dispara() {
        let mut r = router();
        assert!(matches!(r.on_home_cursor(2500, 500), Action::Enter { ref machine, .. } if machine == "win2"));
    }

    #[test]
    fn salto_impossivel_e_descartado() {
        assert!(plausible_delta(30, -12));
        assert!(plausible_delta(-MAX_EVENT_DELTA, MAX_EVENT_DELTA));
        assert!(!plausible_delta(4279, 0), "corrida do park: borda menos centro");
        assert!(!plausible_delta(0, -MAX_EVENT_DELTA - 1));
    }
}
