//! Lado do outro computador, sem o editor de telas: no Linux o servidor não tem **Arrange screens…**,
//! então `mousex side left|right` e os botões da janela de status encostam a máquina inteira à esquerda
//! ou à direita da casa. Máquina nova continua entrando à direita.
use crate::{
    config::{Config, Role, data_dir},
    server::{read_saved, save_layout},
};
use mousex_core::{Monitor, ScreenLayout};
use std::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// `mousex side left|right [MÁQUINA]`. Com o agente no ar, ele mesmo troca na hora (pela porta da janela de
/// status); parado, o layout é gravado aqui e vale quando ele subir.
pub fn cli(args: &[String]) -> Result<(), Box<dyn Error>> {
    let side = args.first().and_then(|word| Side::parse(word)).ok_or("usage: mousex side left|right [MACHINE]")?;
    let name = args[1..].join(" ");
    let name = Some(name.trim()).filter(|name| !name.is_empty());
    let machine = match crate::monitor::ask_side(name, side) {
        Some(reply) => reply?,
        None => set_side(name, side)?,
    };
    println!("{machine} is now on the {} of this computer.", side.name());
    Ok(())
}

/// Grava o lado no layout; sem nome, vale o único outro computador. O agente relê o layout com
/// `server::reload_layout` ou quando sobe.
pub fn set_side(machine: Option<&str>, side: Side) -> Result<String, Box<dyn Error>> {
    let dir = data_dir();
    let home = Config::load(&dir)
        .filter(|config| config.role == Role::Server)
        .ok_or("run this on the main computer (mousex install --role server)")?
        .machine;
    let path = dir.join("layout.json");
    let layout = read_saved(&path);
    let names = others(&layout, &home);
    let machine = match (machine, names.as_slice()) {
        (Some(name), _) if names.iter().any(|known| known == name) => name.to_string(),
        (Some(name), _) => return Err(format!("{name} is not in the layout (known: {})", names.join(", ")).into()),
        (None, [only]) => only.clone(),
        (None, []) => return Err("no other computer in the layout yet: connect one first".into()),
        (None, _) => return Err(format!("more than one computer, say which: {}", names.join(", ")).into()),
    };
    let placed = place_beside(&layout, &home, &machine, side).ok_or("another computer is already on that side")?;
    save_layout(&path, &placed)?;
    Ok(machine)
}

/// Para a janela de status: cada outro computador e o lado em que está (`None`: nem só de um lado).
pub fn sides() -> Vec<(String, Option<Side>)> {
    let dir = data_dir();
    let Some(home) = Config::load(&dir).filter(|config| config.role == Role::Server).map(|config| config.machine)
    else {
        return Vec::new();
    };
    let layout = read_saved(&dir.join("layout.json"));
    others(&layout, &home)
        .into_iter()
        .map(|name| {
            let side = side_of(&layout, &home, &name);
            (name, side)
        })
        .collect()
}

fn others(layout: &ScreenLayout, home: &str) -> Vec<String> {
    let mut names: Vec<String> =
        layout.monitores.iter().filter(|m| m.machine != home).map(|m| m.machine.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Todas as telas da máquina encostam no lado pedido da casa, sem mudar o arranjo entre elas. O topo da
/// tela que encosta fica no topo da tela da casa naquela borda: as duas dividem a borda por onde o cursor passa.
pub fn place_beside(layout: &ScreenLayout, home: &str, machine: &str, side: Side) -> Option<ScreenLayout> {
    let homes = layout.monitores.iter().filter(|m| m.machine == home);
    let theirs = layout.monitores.iter().filter(|m| m.machine == machine);
    let leftmost = |m: &&Monitor| (m.x, m.y);
    let rightmost = |m: &&Monitor| (m.x + m.width, -m.y);
    let (dx, dy) = match side {
        Side::Left => {
            let (anchor, entry) = (homes.min_by_key(leftmost)?, theirs.max_by_key(rightmost)?);
            (anchor.x - entry.x - entry.width, anchor.y - entry.y)
        }
        Side::Right => {
            let (anchor, entry) = (homes.max_by_key(rightmost)?, theirs.min_by_key(leftmost)?);
            (anchor.x + anchor.width - entry.x, anchor.y - entry.y)
        }
    };
    let moved = layout
        .monitores
        .iter()
        .map(|m| if m.machine == machine { Monitor { x: m.x + dx, y: m.y + dy, ..m.clone() } } else { m.clone() });
    let placed = ScreenLayout { monitores: moved.collect() };
    placed.validate().is_ok().then_some(placed)
}

/// De que lado da casa estão todas as telas da máquina.
pub fn side_of(layout: &ScreenLayout, home: &str, machine: &str) -> Option<Side> {
    let ((home_left, home_right), (left, right)) = (span(layout, home)?, span(layout, machine)?);
    if right <= home_left {
        Some(Side::Left)
    } else if left >= home_right {
        Some(Side::Right)
    } else {
        None
    }
}

/// Coluna da primeira tela e coluna logo depois da última, somando todas as telas da máquina.
fn span(layout: &ScreenLayout, machine: &str) -> Option<(i32, i32)> {
    let mine = || layout.monitores.iter().filter(|m| m.machine == machine);
    Some((mine().map(|m| m.x).min()?, mine().map(|m| m.x + m.width).max()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tela(machine: &str, id: &str, x: i32, y: i32, width: i32, height: i32) -> Monitor {
        Monitor { id: format!("{machine}:{id}"), machine: machine.into(), x, y, width, height, scale_fator: 1.0 }
    }

    /// Casa com duas telas desalinhadas e o Mac à direita, como num layout real.
    fn mesa() -> ScreenLayout {
        ScreenLayout {
            monitores: vec![
                tela("casa", "1", 0, 0, 1920, 1200),
                tela("casa", "2", 1920, 120, 1920, 1080),
                tela("mac", "1", 3840, 0, 2056, 1329),
            ],
        }
    }

    fn onde(layout: &ScreenLayout, id: &str) -> (i32, i32) {
        let m = layout.monitores.iter().find(|m| m.id == id).expect("tela");
        (m.x, m.y)
    }

    #[test]
    fn esquerda_encosta_na_primeira_tela_e_direita_volta_para_a_ultima() {
        assert_eq!(side_of(&mesa(), "casa", "mac"), Some(Side::Right));
        let left = place_beside(&mesa(), "casa", "mac", Side::Left).expect("cabe à esquerda");
        assert_eq!(onde(&left, "mac:1"), (-2056, 0));
        assert_eq!(side_of(&left, "casa", "mac"), Some(Side::Left));
        let right = place_beside(&left, "casa", "mac", Side::Right).expect("cabe à direita");
        assert_eq!(onde(&right, "mac:1"), (3840, 120));
        assert_eq!(side_of(&right, "casa", "mac"), Some(Side::Right));
    }

    #[test]
    fn telas_do_cliente_mudam_de_lado_juntas() {
        let mut layout = mesa();
        layout.monitores.push(tela("mac", "2", 5896, 0, 1280, 1024));
        let left = place_beside(&layout, "casa", "mac", Side::Left).expect("cabe à esquerda");
        assert_eq!((onde(&left, "mac:1"), onde(&left, "mac:2")), ((-3336, 0), (-1280, 0)));
    }

    #[test]
    fn lado_ocupado_por_outro_computador_e_recusado() {
        let mut layout = mesa();
        layout.monitores.push(tela("pc", "1", -1920, 0, 1920, 1080));
        assert!(place_beside(&layout, "casa", "mac", Side::Left).is_none());
        assert_eq!(Side::parse("left"), Some(Side::Left));
        assert_eq!(Side::parse("esquerda"), None);
    }
}
