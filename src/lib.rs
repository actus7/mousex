pub mod discovery;
pub mod i18n;
pub mod identity;
pub mod keymap;
pub mod net;
pub mod pairing;
pub mod proto;
pub mod router;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const MAX_EVENT_BYTES: usize = 4 * 1024;
/// Limite do futuro transporte de clipboard; clipboard não integra InputEvent.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: String,
    pub machine: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale_fator: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Monitor {
    /// Layout usa pixels globais; coordenadas locais usam unidades lógicas.
    pub fn bounds(&self) -> Bounds {
        Bounds {
            left: self.x as f64,
            top: self.y as f64,
            right: self.x as f64 + self.width as f64,
            bottom: self.y as f64 + self.height as f64,
        }
    }
    pub fn global_to_local(&self, p: Point) -> Point {
        Point { x: (p.x - self.x as f64) / self.scale_fator, y: (p.y - self.y as f64) / self.scale_fator }
    }
    pub fn local_to_global(&self, p: Point) -> Point {
        Point { x: self.x as f64 + p.x * self.scale_fator, y: self.y as f64 + p.y * self.scale_fator }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenLayout {
    pub monitores: Vec<Monitor>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    InvalidDimensions(String),
    InvalidScale(String),
    InvalidIdentity(String),
    Overlap(String, String),
}
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for LayoutError {}
impl ScreenLayout {
    pub fn validate(&self) -> Result<(), LayoutError> {
        let mut ids = BTreeSet::new();
        for (i, m) in self.monitores.iter().enumerate() {
            if m.id.is_empty() || m.machine.is_empty() || !ids.insert(&m.id) {
                return Err(LayoutError::InvalidIdentity(m.id.clone()));
            }
            if m.width <= 0 || m.height <= 0 {
                return Err(LayoutError::InvalidDimensions(m.id.clone()));
            }
            if !m.scale_fator.is_finite() || !(0.5..=4.0).contains(&m.scale_fator) {
                return Err(LayoutError::InvalidScale(m.id.clone()));
            }
            let a = m.bounds();
            for n in &self.monitores[..i] {
                let b = n.bounds();
                if a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom {
                    return Err(LayoutError::Overlap(n.id.clone(), m.id.clone()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
#[derive(Debug, PartialEq)]
pub struct Transition {
    pub monitor_id: String,
    pub machine: String,
    pub global: Point,
    pub local: Point,
}

/// Cursor deve estar exatamente na borda de saída. Cantos sem segmento compartilhado não conectam.
pub fn edge_router(
    layout: &ScreenLayout,
    cursor: Point,
    direction: Direction,
) -> Result<Option<Transition>, LayoutError> {
    layout.validate()?;
    if !cursor.x.is_finite() || !cursor.y.is_finite() {
        return Ok(None);
    }
    for source in &layout.monitores {
        let a = source.bounds();
        for target in &layout.monitores {
            if source.id == target.id {
                continue;
            }
            let b = target.bounds();
            let vertical = cursor.y >= a.top.max(b.top) && cursor.y < a.bottom.min(b.bottom);
            let horizontal = cursor.x >= a.left.max(b.left) && cursor.x < a.right.min(b.right);
            let adjacent = match direction {
                Direction::Right => cursor.x == a.right && a.right == b.left && vertical,
                Direction::Left => cursor.x == a.left && a.left == b.right && vertical,
                Direction::Down => cursor.y == a.bottom && a.bottom == b.top && horizontal,
                Direction::Up => cursor.y == a.top && a.top == b.bottom && horizontal,
            };
            if adjacent {
                // ponytail: entrada usa um pixel global para manter bounds semiabertos; subpixel fica para captura nativa.
                let global = match direction {
                    Direction::Left => Point { x: b.right - 1.0, ..cursor },
                    Direction::Up => Point { y: b.bottom - 1.0, ..cursor },
                    _ => cursor,
                };
                return Ok(Some(Transition {
                    monitor_id: target.id.clone(),
                    machine: target.machine.clone(),
                    global,
                    local: target.global_to_local(global),
                }));
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Keyboard { code: u32, pressed: bool },
    MouseMove { x: i32, y: i32 },
    Button { button: u16, pressed: bool },
    Scroll { x: i32, y: i32 },
}
#[derive(Debug, PartialEq, Eq)]
pub enum EventError {
    TooLarge,
    InvalidEncoding,
}
impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for EventError {}
impl InputEvent {
    /// Tag u8 (1..4), inteiros little-endian, bool 0/1; comprimento exato obrigatório.
    pub fn encode(&self) -> Result<Vec<u8>, EventError> {
        let mut bytes = Vec::new();
        match *self {
            Self::Keyboard { code, pressed } => {
                bytes.push(1);
                bytes.extend(code.to_le_bytes());
                bytes.push(u8::from(pressed));
            }
            Self::MouseMove { x, y } | Self::Scroll { x, y } => {
                bytes.push(if matches!(self, Self::MouseMove { .. }) { 2 } else { 4 });
                bytes.extend(x.to_le_bytes());
                bytes.extend(y.to_le_bytes());
            }
            Self::Button { button, pressed } => {
                bytes.push(3);
                bytes.extend(button.to_le_bytes());
                bytes.push(u8::from(pressed));
            }
        }
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(EventError::TooLarge);
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, EventError> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(EventError::TooLarge);
        }
        match bytes {
            [1, a, b, c, d, pressed @ 0..=1] => {
                Ok(Self::Keyboard { code: u32::from_le_bytes([*a, *b, *c, *d]), pressed: *pressed == 1 })
            }
            [tag @ (2 | 4), a, b, c, d, e, f, g, h] => {
                let x = i32::from_le_bytes([*a, *b, *c, *d]);
                let y = i32::from_le_bytes([*e, *f, *g, *h]);
                Ok(if *tag == 2 { Self::MouseMove { x, y } } else { Self::Scroll { x, y } })
            }
            [3, a, b, pressed @ 0..=1] => {
                Ok(Self::Button { button: u16::from_le_bytes([*a, *b]), pressed: *pressed == 1 })
            }
            _ => Err(EventError::InvalidEncoding),
        }
    }
}

#[derive(Default)]
pub struct KeyStateTracker {
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u16>,
}
impl KeyStateTracker {
    pub fn apply(&mut self, event: &InputEvent) {
        match *event {
            InputEvent::Keyboard { code, pressed } => {
                if pressed {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            InputEvent::Button { button, pressed } => {
                if pressed {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
            }
            _ => {}
        }
    }
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut events: Vec<_> = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|code| InputEvent::Keyboard { code, pressed: false })
            .collect();
        events.extend(
            std::mem::take(&mut self.buttons).into_iter().map(|button| InputEvent::Button { button, pressed: false }),
        );
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout() -> ScreenLayout {
        ScreenLayout {
            monitores: vec![
                Monitor {
                    id: "a".into(),
                    machine: "one".into(),
                    x: -100,
                    y: -50,
                    width: 100,
                    height: 100,
                    scale_fator: 1.0,
                },
                Monitor {
                    id: "b".into(),
                    machine: "two".into(),
                    x: 0,
                    y: -50,
                    width: 100,
                    height: 100,
                    scale_fator: 2.0,
                },
            ],
        }
    }
    #[test]
    fn overlap() {
        let mut l = layout();
        l.monitores[1].x = -1;
        assert!(matches!(l.validate(), Err(LayoutError::Overlap(..))));
    }
    #[test]
    fn dimensions() {
        for size in [0, -1] {
            let mut l = layout();
            l.monitores[0].width = size;
            assert!(matches!(l.validate(), Err(LayoutError::InvalidDimensions(_))));
        }
    }
    #[test]
    fn scale() {
        for scale in [0.4, 4.1, f64::NAN] {
            let mut l = layout();
            l.monitores[0].scale_fator = scale;
            assert!(matches!(l.validate(), Err(LayoutError::InvalidScale(_))));
        }
    }
    #[test]
    fn identity() {
        let mut l = layout();
        l.monitores[1].id = "a".into();
        assert!(matches!(l.validate(), Err(LayoutError::InvalidIdentity(_))));
    }
    #[test]
    fn geometry() {
        let l = layout();
        let m = &l.monitores[1];
        let p = Point { x: 20.0, y: -30.0 };
        assert_eq!(m.global_to_local(p), Point { x: 10.0, y: 10.0 });
        assert_eq!(m.local_to_global(m.global_to_local(p)), p);
    }
    #[test]
    fn routing() {
        let l = layout();
        let t = edge_router(&l, Point { x: 0.0, y: -30.0 }, Direction::Right).expect("layout").expect("neighbor");
        assert_eq!(t.monitor_id, "b");
        assert_eq!(t.local, Point { x: 0.0, y: 10.0 });
        let t = edge_router(&l, Point { x: 0.0, y: -30.0 }, Direction::Left).expect("layout").expect("neighbor");
        assert_eq!(t.global.x, -1.0);
    }
    #[test]
    fn no_wrap_gap_or_corner() {
        let mut l = layout();
        assert!(edge_router(&l, Point { x: -100.0, y: 0.0 }, Direction::Left).expect("layout").is_none());
        l.monitores[1].x = 1;
        assert!(edge_router(&l, Point { x: 0.0, y: 0.0 }, Direction::Right).expect("layout").is_none());
        l.monitores[1].x = 0;
        l.monitores[1].y = 50;
        assert!(edge_router(&l, Point { x: 0.0, y: 50.0 }, Direction::Right).expect("layout").is_none());
    }
    #[test]
    fn vertical_routing() {
        let mut l = layout();
        l.monitores[1].x = -100;
        l.monitores[1].y = 50;
        assert!(edge_router(&l, Point { x: -40.0, y: 50.0 }, Direction::Down).expect("layout").is_some());
        assert!(edge_router(&l, Point { x: -40.0, y: 50.0 }, Direction::Up).expect("layout").is_some());
    }
    #[test]
    fn roundtrip() {
        for e in [
            InputEvent::Keyboard { code: 42, pressed: true },
            InputEvent::Keyboard { code: 42, pressed: false },
            InputEvent::MouseMove { x: -2, y: 3 },
            InputEvent::Button { button: 1, pressed: true },
            InputEvent::Scroll { x: 0, y: -120 },
        ] {
            assert_eq!(InputEvent::decode(&e.encode().expect("encode")), Ok(e));
        }
        assert_eq!(InputEvent::Keyboard { code: 42, pressed: true }.encode(), Ok(vec![1, 42, 0, 0, 0, 1]));
    }
    #[test]
    fn invalid_payload() {
        assert_eq!(InputEvent::decode(&vec![0; MAX_EVENT_BYTES + 1]), Err(EventError::TooLarge));
        for bytes in [vec![], vec![1, 0, 0, 0, 0, 2], vec![9], vec![3, 0, 0, 1, 0]] {
            assert_eq!(InputEvent::decode(&bytes), Err(EventError::InvalidEncoding));
        }
    }
    #[test]
    fn releases() {
        let mut tracker = KeyStateTracker::default();
        tracker.apply(&InputEvent::Keyboard { code: 42, pressed: true });
        tracker.apply(&InputEvent::Keyboard { code: 42, pressed: true });
        tracker.apply(&InputEvent::Keyboard { code: 7, pressed: true });
        tracker.apply(&InputEvent::Keyboard { code: 7, pressed: false });
        tracker.apply(&InputEvent::Button { button: 1, pressed: true });
        assert_eq!(
            tracker.release_all(),
            vec![InputEvent::Keyboard { code: 42, pressed: false }, InputEvent::Button { button: 1, pressed: false }]
        );
        assert!(tracker.release_all().is_empty());
    }
}
