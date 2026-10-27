//! Live snapshot the status window reads from `activity.json`.
//! Writes stay off until [`enable`], so unit tests do not touch the real data directory.

use crate::config::data_dir;
use serde::{Deserialize, Serialize};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

static ENABLED: AtomicBool = AtomicBool::new(false);
static LAST_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Live {
    pub phase: String,
    pub detail: String,
    pub pointer: String,
    pub input: String,
    pub at: u64,
}

impl Default for Live {
    fn default() -> Self {
        Self { phase: "idle".into(), detail: String::new(), pointer: String::new(), input: String::new(), at: 0 }
    }
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Replace the current phase. `detail` is extra text, such as the server address.
pub fn set(phase: &str, detail: &str) {
    update(
        |live| {
            live.phase = phase.to_string();
            live.detail = detail.to_string();
        },
        true,
    );
}

/// Latest remote pointer, as `monitor:x:y` (monitor is a zero-based index).
pub fn pointer(text: &str) {
    let text = text.to_string();
    update(|live| live.pointer = text, false);
}

/// Latest button, key, wheel or release. Codes are translated by the status window.
pub fn input(text: &str) {
    let text = text.to_string();
    update(|live| live.input = text, true);
}

pub fn read() -> Option<Live> {
    let bytes = std::fs::read(data_dir().join("activity.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn state() -> &'static Mutex<Live> {
    static STATE: OnceLock<Mutex<Live>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Live::default()))
}

fn update(change: impl FnOnce(&mut Live), force: bool) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    if !force && now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) < 200 {
        return;
    }
    let Ok(mut live) = state().lock() else { return };
    change(&mut live);
    live.at = now / 1000;
    LAST_MS.store(now, Ordering::Relaxed);
    let path = data_dir().join("activity.json");
    let tmp = data_dir().join("activity.json.tmp");
    if let Ok(body) = serde_json::to_vec(&*live)
        && std::fs::write(&tmp, body).is_ok()
    {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
