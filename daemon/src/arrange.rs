//! Arrumar telas no Linux: o editor do Windows (Tauri) não roda aqui, então a janela de status serve uma versão
//! web dele em `/arrange` (`mousex arrange`). As telas deste computador ficam onde o desktop pôs; as dos outros
//! computadores se arrastam em volta delas. Salvar vale na hora, sem reiniciar o agente.
use crate::{
    config::{Config, Role, data_dir},
    server::{read_saved, save_layout},
};
use mousex_core::{
    LayoutError, Monitor, ScreenLayout,
    i18n::{Lang, lang},
};
use std::error::Error;

/// `GET /api/layout`: o layout salvo, qual máquina é esta, quem está conectado e o idioma.
pub fn layout_json() -> String {
    let dir = data_dir();
    let home = Config::load(&dir).filter(|config| config.role == Role::Server).map(|config| config.machine);
    serde_json::json!({
        "home": home,
        "portuguese": lang() == Lang::Pt,
        "connected": crate::os::connected_machines(),
        "layout": read_saved(&dir.join("layout.json")),
    })
    .to_string()
}

/// `POST /api/layout`: grava o que o editor mandou; o agente deste processo relê o layout na hora.
pub fn save_request(allowed: bool, body: &[u8]) -> String {
    let lang = lang();
    let result = if allowed { save(body) } else { Err("forbidden".into()) };
    match result {
        Ok(true) => reply(true, lang.pick("Layout salvo. Já está valendo.", "Layout saved. It applies now.")),
        Ok(false) => reply(
            true,
            lang.pick("Layout salvo. Vale quando o MouseX iniciar.", "Layout saved. It applies when MouseX starts."),
        ),
        Err(error) => reply(false, &error.to_string()),
    }
}

fn reply(ok: bool, message: &str) -> String {
    serde_json::json!({ "ok": ok, "message": message }).to_string()
}

fn save(body: &[u8]) -> Result<bool, Box<dyn Error>> {
    let edited: ScreenLayout = serde_json::from_slice(body)?;
    let dir = data_dir();
    let home = Config::load(&dir)
        .filter(|config| config.role == Role::Server)
        .ok_or("run this on the main computer (mousex install --role server)")?
        .machine;
    let path = dir.join("layout.json");
    let layout = merge_edit(&read_saved(&path), &edited, &home);
    if let Err(error) = layout.validate() {
        let message = match error {
            LayoutError::Overlap(..) => lang().pick(
                "Telas sobrepostas. Separe as telas antes de salvar.",
                "Screens overlap. Move them apart before saving.",
            ),
            _ => lang().pick("Layout inválido.", "Invalid layout."),
        };
        return Err(message.into());
    }
    save_layout(&path, &layout)?;
    Ok(crate::server::reload_layout())
}

/// Telas desta máquina ficam como estão (quem manda nelas é o desktop). Dos outros computadores, só telas que
/// já estavam no layout: o editor move, redimensiona e remove, mas não inventa tela.
fn merge_edit(saved: &ScreenLayout, edited: &ScreenLayout, home: &str) -> ScreenLayout {
    let homes = saved.monitores.iter().filter(|m| m.machine == home).cloned();
    let known = |m: &&Monitor| {
        m.machine != home && saved.monitores.iter().any(|old| old.id == m.id && old.machine == m.machine)
    };
    ScreenLayout { monitores: homes.chain(edited.monitores.iter().filter(known).cloned()).collect() }
}

pub const PAGE: &str = r##"<!DOCTYPE html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MouseX · Arrumar telas</title>
<link rel="icon" href="/icon.png">
<style>
  :root { font-family: system-ui, sans-serif; color: #e1e8e8; background: #101416; color-scheme: dark; --panel: #171d20; --border: #2a3438; --muted: #96a6ad; --accent: #9ee8c3; }
  * { box-sizing: border-box; }
  body { margin: 0; background: #101416; }
  button, input { font: inherit; }
  button { cursor: pointer; border: 1px solid var(--border); border-radius: 7px; background: #20292d; color: inherit; padding: 10px 15px; transition: background .15s, border-color .15s; }
  button:hover { background: #2b373b; border-color: #667a81; }
  button:disabled { opacity: .45; cursor: not-allowed; }
  button:focus-visible, input:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  button[aria-pressed="true"] { border-color: var(--accent); color: var(--accent); }
  input { width: 100%; border: 1px solid #354247; border-radius: 6px; background: #101719; padding: 10px; color: #e1e8e8; }
  input:disabled { opacity: .5; }
  label { display: block; font-size: 12px; color: #aebdc2; }
  label input { margin-top: 7px; }
  h1 { font-size: 32px; letter-spacing: -1.3px; margin: 9px 0 12px; font-weight: 700; }
  h2 { font-size: 19px; letter-spacing: -.5px; overflow-wrap: anywhere; margin-bottom: 22px; }
  p { color: var(--muted); font-size: 14px; line-height: 1.65; }
  header { height: 78px; border-bottom: 1px solid var(--border); display: flex; align-items: center; gap: 28px; padding: 0 36px; }
  .brand { color: #eaf7f1; font-size: 26px; font-weight: 800; display: flex; align-items: center; gap: 12px; }
  .tag, .eyebrow { font-size: 10px; font-weight: 700; letter-spacing: 1.8px; color: var(--muted); }
  main { padding: 38px 36px; }
  .page-title { display: flex; align-items: center; justify-content: space-between; gap: 24px; margin-bottom: 26px; }
  .page-title p { max-width: 660px; margin-bottom: 0; }
  .actions { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  .primary { background: var(--accent); color: #11291e; border-color: var(--accent); font-weight: 700; white-space: nowrap; }
  .primary:hover { background: #bff4d9; border-color: #bff4d9; }
  .workspace { display: grid; grid-template-columns: minmax(0, 1fr) 258px; border: 1px solid var(--border); border-radius: 10px; overflow: hidden; background: var(--panel); }
  .toolbar { display: flex; align-items: center; padding: 16px 18px; border-bottom: 1px solid var(--border); font-size: 12px; color: #b9c8cd; flex-wrap: wrap; gap: 8px; }
  .legend { display: inline-flex; align-items: center; gap: 6px; margin-right: 16px; color: var(--muted); }
  .legend i { width: 10px; height: 10px; border-radius: 3px; display: inline-block; }
  .canvas { height: 430px; overflow: auto; background-color: #111719; background-image: radial-gradient(#344044 .8px, transparent .8px); background-size: 18px 18px; position: relative; }
  .world { position: relative; min-width: 100%; min-height: 100%; }
  .monitor { position: absolute; touch-action: none; user-select: none; display: flex; flex-direction: column; justify-content: center; align-items: center; gap: 6px; border: 1px solid #53656d; border-radius: 8px; background: #222e33; box-shadow: 0 8px 16px #0003; padding: 8px; overflow: hidden; min-width: 18px; min-height: 18px; cursor: grab; }
  .monitor:active { cursor: grabbing; }
  .monitor.fixed { cursor: default; }
  .monitor.selected { border-width: 2px; background: #213b33; box-shadow: 0 0 0 4px #9ee8c30c; }
  .monitor.conflict { border-color: #fa9b92 !important; background: #4c2929; }
  .monitor strong { font-size: 13px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .monitor .res { font-size: 10px; color: #acc1c6; }
  .monitor .number { font-size: 23px; font-weight: 500; color: #748c93; }
  .monitor.selected .number { color: var(--accent); }
  .monitor b { font-size: 10px; color: #ffc3bc; }
  .monitor b:empty { display: none; }
  .canvas-footer { padding: 13px 16px; display: flex; align-items: center; justify-content: space-between; gap: 10px; font-size: 11px; color: var(--muted); border-top: 1px solid var(--border); flex-wrap: wrap; }
  .canvas-footer label { display: flex; align-items: center; gap: 8px; }
  .canvas-footer input { width: 150px; margin: 0; padding: 0; }
  aside { border-left: 1px solid var(--border); padding: 22px 20px; }
  aside label { margin-bottom: 15px; }
  .fields { display: grid; grid-template-columns: 1fr 1fr; gap: 0 12px; min-width: 0; }
  .fields input { min-width: 0; }
  .hint { font-size: 11px; margin: 18px 0; }
  .danger { color: #f4aaa2; background: transparent; border-color: #60403e; font-size: 12px; width: 100%; }
  .feedback { min-height: 38px; margin-top: 16px; font-size: 12px; color: var(--muted); line-height: 1.6; }
  .feedback.error { color: #ffb4a8; }
  @media (max-width: 850px) {
    header { padding: 0 20px; } .tag { display: none; } main { padding: 26px 20px; }
    .workspace { grid-template-columns: 1fr; } .canvas { height: 360px; }
    aside { border-left: 0; border-top: 1px solid var(--border); }
    .page-title { flex-direction: column; align-items: flex-start; } h1 { font-size: 27px; }
  }
  @media (prefers-reduced-motion: reduce) { * { transition: none !important; } }
</style>
</head>
<body>
<header><span class="brand"><img src="/icon.png" alt="" width="30" height="30"><span>MouseX</span></span><span class="tag" id="tag">ARRUMAR TELAS</span></header>
<main>
  <div class="page-title">
    <div>
      <div class="eyebrow" id="eyebrow">LAYOUT DA SUA MESA</div>
      <h1 id="title">Arrume as telas como estão na mesa</h1>
      <p id="lead"></p>
    </div>
    <div class="actions">
      <button type="button" id="close">Fechar</button>
      <button type="button" id="only" aria-pressed="false">Só telas conectadas</button>
      <button type="button" id="save" class="primary" disabled>Salvar</button>
    </div>
  </div>
  <div class="workspace">
    <section aria-label="Telas">
      <div class="toolbar" id="legend"></div>
      <div class="canvas" id="canvas"><div class="world" id="world"></div></div>
      <div class="canvas-footer">
        <span id="keys"></span>
        <label>Zoom <input type="range" id="zoom" min="0.03" max="0.3" step="0.01" value="0.17"></label>
      </div>
    </section>
    <aside id="panel"></aside>
  </div>
  <div class="feedback" id="feedback" role="status">Carregando o layout…</div>
</main>
<script>
const $ = (id) => document.getElementById(id);
let pt = (navigator.language || "").toLowerCase().startsWith("pt");
const t = (a, b) => (pt ? a : b);
const palette = ["#9ee8c3", "#8ec5ff", "#ffc98e", "#e7a1ff", "#ff9e9e", "#d8e88e"];
const state = { layout: { monitores: [] }, home: null, connected: new Set(), selected: null, dirty: false, zoom: 0.17, only: false, drag: null, message: "" };
const nodes = new Map();

// Mesmas regras do editor do Windows (app/src/layout.js).
function overlapping(list) {
  const ids = new Set();
  for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
    const a = list[i], b = list[j];
    if (a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y) { ids.add(a.id); ids.add(b.id); }
  }
  return ids;
}
function snap(m, others, threshold) {
  let x = m.x, y = m.y, dx = threshold, dy = threshold;
  for (const b of others) {
    for (const edge of [b.x, b.x + b.width, b.x - m.width, b.x + b.width - m.width]) if (Math.abs(edge - m.x) < dx) { dx = Math.abs(edge - m.x); x = edge; }
    for (const edge of [b.y, b.y + b.height, b.y - m.height, b.y + b.height - m.height]) if (Math.abs(edge - m.y) < dy) { dy = Math.abs(edge - m.y); y = edge; }
  }
  return { ...m, x, y };
}
// Encostam quando dividem um trecho de borda (só o canto não basta): é por ali que o cursor passa.
const touching = (a, b) =>
  ((a.x + a.width === b.x || b.x + b.width === a.x) && a.y < b.y + b.height && b.y < a.y + a.height) ||
  ((a.y + a.height === b.y || b.y + b.height === a.y) && a.x < b.x + b.width && b.x < a.x + a.width);
// Computadores aonde o cursor não chega, saindo das telas desta máquina de borda em borda.
function unreachable(list) {
  if (!state.home) return [];
  const reached = new Set(list.filter((m) => m.machine === state.home));
  for (const m of reached) for (const o of list) if (!reached.has(o) && touching(m, o)) reached.add(o);
  return [...new Set(list.filter((m) => !reached.has(m)).map((m) => m.machine))];
}
function resize(m, scale) {
  const s = Math.min(4, Math.max(0.5, scale));
  const nativeW = m.width / m.scale_fator, nativeH = m.height / m.scale_fator;
  return { ...m, width: Math.round(nativeW * s), height: Math.round(nativeH * s), scale_fator: s };
}
function fitZoom(list, available) {
  if (!list.length) return 0.17;
  const left = Math.min(...list.map((m) => m.x)), right = Math.max(...list.map((m) => m.x + m.width));
  return Math.max(0.03, Math.min(0.17, (available - 160) / (right - left)));
}
function validate(list) {
  for (const m of list) {
    if (![m.x, m.y, m.width, m.height].every(Number.isInteger) || m.width <= 0 || m.height <= 0 || !(m.scale_fator >= 0.5 && m.scale_fator <= 4)) {
      return t("Use posições inteiras e escala entre 0,5 e 4.", "Use whole-number positions and a scale between 0.5 and 4.");
    }
  }
  return overlapping(list).size ? t("Telas sobrepostas. Separe as telas antes de salvar.", "Screens overlap. Move them apart before saving.") : "";
}

const machines = () => [...new Set(state.layout.monitores.map((m) => m.machine))];
const color = (machine) => palette[machines().indexOf(machine) % palette.length];
const isHome = (m) => m.machine === state.home;
const online = (machine) => machine === state.home || state.connected.has(machine);
const native = (m) => Math.round(m.width / m.scale_fator) + " × " + Math.round(m.height / m.scale_fator);
const visible = () => state.layout.monitores.filter((m) => !state.only || online(m.machine));
const find = (id) => state.layout.monitores.find((m) => m.id === id);
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function update(id, change) {
  state.layout.monitores = state.layout.monitores.map((m) => (m.id === id ? change(m) : m));
  state.dirty = true;
  state.message = "";
  paint();
}

function makeNode(id) {
  const node = el("button", "monitor");
  node.type = "button";
  node.append(el("span", "number"), el("strong"), el("span", "res"), el("b"));
  node.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    select(id);
    const m = find(id);
    if (!m || isHome(m)) return;
    try { node.setPointerCapture(event.pointerId); } catch {}
    state.drag = { id, x: m.x, y: m.y, px: event.clientX, py: event.clientY };
  });
  node.addEventListener("keydown", (event) => {
    const dir = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[event.key];
    const m = find(id);
    if (!dir || !m || isHome(m)) return;
    event.preventDefault();
    const step = event.shiftKey ? 10 : 1;
    update(id, (o) => ({ ...o, x: o.x + dir[0] * step, y: o.y + dir[1] * step }));
  });
  return node;
}

function paint() {
  const list = visible();
  const conflict = overlapping(state.layout.monitores);
  const error = validate(state.layout.monitores);
  const zoom = state.zoom;
  const ox = 80 - Math.min(0, ...list.map((m) => m.x)) * zoom;
  const oy = 80 - Math.min(0, ...list.map((m) => m.y)) * zoom;
  $("legend").replaceChildren(...machines().map((name) => {
    const item = el("span", "legend");
    const swatch = el("i");
    swatch.style.background = color(name);
    item.append(swatch, document.createTextNode(online(name) ? name : name + " · " + t("desconectado", "offline")));
    return item;
  }));
  const world = $("world");
  world.style.width = Math.max(900, ...list.map((m) => (m.x + m.width) * zoom + ox + 80)) + "px";
  world.style.height = Math.max(420, ...list.map((m) => (m.y + m.height) * zoom + oy + 80)) + "px";
  const order = [...list].sort((a, b) => a.x - b.x || a.y - b.y).map((m) => m.id);
  for (const [id, node] of nodes) if (!list.some((m) => m.id === id)) { node.remove(); nodes.delete(id); }
  for (const m of list) {
    let node = nodes.get(m.id);
    if (!node) { node = makeNode(m.id); nodes.set(m.id, node); world.append(node); }
    node.className = "monitor" + (state.selected === m.id ? " selected" : "") + (conflict.has(m.id) ? " conflict" : "") + (isHome(m) ? " fixed" : "");
    Object.assign(node.style, { left: ox + m.x * zoom + "px", top: oy + m.y * zoom + "px", width: m.width * zoom + "px", height: m.height * zoom + "px", borderColor: color(m.machine) });
    const [number, name, res, warn] = node.children;
    number.textContent = order.indexOf(m.id) + 1;
    name.textContent = m.machine;
    name.style.color = color(m.machine);
    res.textContent = native(m);
    warn.textContent = conflict.has(m.id) ? t("Sobreposta", "Overlapping") : "";
    node.setAttribute("aria-label", m.machine + ", " + native(m) + (conflict.has(m.id) ? t(", sobreposta", ", overlapping") : ""));
  }
  panelValues();
  $("save").disabled = !!error || !state.dirty;
  $("close").textContent = state.dirty ? t("Fechar sem salvar", "Close without saving") : t("Fechar", "Close");
  $("only").setAttribute("aria-pressed", String(state.only));
  const lonely = unreachable(state.layout.monitores);
  const warn = lonely.length ? t("Sem caminho até ", "No way to reach ") + lonely.join(", ") + t(": encoste uma borda numa tela vizinha.", ": make one of its edges touch a neighboring screen.") : "";
  const feedback = $("feedback");
  feedback.className = error || warn ? "feedback error" : "feedback";
  feedback.textContent = error || [state.message, warn].filter(Boolean).join(" ") || (state.dirty ? t("Alterações não salvas.", "Unsaved changes.") : t("Bordas se encaixam sozinhas ao arrastar.", "Edges snap together while you drag."));
}

function select(id) {
  if (state.selected === id) return;
  state.selected = id;
  buildPanel();
  paint();
}

function buildPanel() {
  const m = find(state.selected);
  const parts = [el("div", "eyebrow", t("TELA SELECIONADA", "SELECTED SCREEN"))];
  if (!m) {
    parts.push(el("p", "", t("Clique numa tela para ajustar.", "Click a screen to adjust it.")));
    return $("panel").replaceChildren(...parts);
  }
  const title = el("h2", "", m.machine);
  title.style.color = color(m.machine);
  const res = el("p", "hint");
  res.id = "panel-res";
  parts.push(title, res);
  if (isHome(m)) {
    parts.push(el("p", "hint", t("Tela deste computador: a posição vem do sistema (Configurações → Telas). Arraste as telas dos outros computadores em volta dela.", "One of this computer's screens: its position comes from the system (Settings → Displays). Drag the other computers' screens around it.")));
  } else {
    const label = el("label", "", t("Tamanho na mesa", "Size on desk"));
    const range = el("input");
    Object.assign(range, { type: "range", min: "0.5", max: "4", step: "0.01", id: "panel-scale" });
    range.addEventListener("input", () => update(m.id, (o) => resize(o, +range.value)));
    const note = el("span", "hint");
    note.id = "panel-scale-note";
    label.append(range, note);
    parts.push(label);
  }
  const fields = el("div", "fields");
  for (const [key, text] of [["x", t("Posição X", "Position X")], ["y", t("Posição Y", "Position Y")]]) {
    const label = el("label", "", text);
    const input = el("input");
    Object.assign(input, { type: "number", step: "1", id: "panel-" + key, disabled: isHome(m) });
    input.addEventListener("input", () => update(m.id, (o) => ({ ...o, [key]: Math.round(Number(input.value) || 0) })));
    label.append(input);
    fields.append(label);
  }
  parts.push(fields);
  if (!isHome(m)) {
    const remove = el("button", "danger", t("Remover tela", "Remove screen"));
    remove.type = "button";
    remove.addEventListener("click", () => {
      state.layout.monitores = state.layout.monitores.filter((o) => o.id !== m.id);
      state.selected = null;
      state.dirty = true;
      buildPanel();
      paint();
    });
    parts.push(remove, el("p", "hint", t("Tela removida volta sozinha quando a máquina reconectar.", "A removed screen comes back when its computer reconnects.")));
  }
  $("panel").replaceChildren(...parts);
}

function panelValues() {
  const m = find(state.selected);
  if (!m) return;
  const set = (id, value) => { const node = $(id); if (node && document.activeElement !== node) node.value = value; };
  set("panel-x", m.x);
  set("panel-y", m.y);
  set("panel-scale", m.scale_fator);
  $("panel-res").textContent = t("Resolução", "Resolution") + " " + native(m) + " (" + m.id + ")";
  const note = $("panel-scale-note");
  if (note) note.textContent = Math.round(m.scale_fator * 100) + "% · " + t("aumente para alinhar com telas fisicamente maiores", "increase it to match physically larger screens");
}

async function save() {
  const error = validate(state.layout.monitores);
  if (error) { state.message = error; return paint(); }
  $("save").disabled = true;
  try {
    const reply = await (await fetch("/api/layout", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(state.layout) })).json();
    state.message = reply.message || "";
    if (reply.ok) state.dirty = false;
  } catch (error) {
    state.message = String(error);
  }
  paint();
}

function texts() {
  document.documentElement.lang = pt ? "pt-BR" : "en";
  document.title = "MouseX · " + t("Arrumar telas", "Arrange screens");
  $("tag").textContent = t("ARRUMAR TELAS", "ARRANGE SCREENS");
  $("eyebrow").textContent = t("LAYOUT DA SUA MESA", "YOUR DESK LAYOUT");
  $("title").textContent = t("Arrume as telas como estão na mesa", "Arrange the screens as they sit on your desk");
  $("lead").textContent = t("Arraste cada tela. O cursor passa entre telas cujas bordas se encostam.", "Drag each screen. The cursor moves between screens whose edges touch.");
  $("only").textContent = t("Só telas conectadas", "Connected screens only");
  $("save").textContent = t("Salvar", "Save");
  $("keys").textContent = t("Arraste para mover · Setas: 1 px · Shift + setas: 10 px", "Drag to move · Arrows: 1 px · Shift + arrows: 10 px");
}

$("canvas").addEventListener("pointermove", (event) => {
  const drag = state.drag;
  if (!drag) return;
  const others = state.layout.monitores.filter((o) => o.id !== drag.id);
  update(drag.id, (m) => snap({ ...m, x: Math.round(drag.x + (event.clientX - drag.px) / state.zoom), y: Math.round(drag.y + (event.clientY - drag.py) / state.zoom) }, others, 12 / state.zoom));
});
for (const type of ["pointerup", "pointercancel"]) $("canvas").addEventListener(type, () => { state.drag = null; });
$("zoom").addEventListener("input", (event) => { state.zoom = +event.target.value; paint(); });
$("only").addEventListener("click", () => { state.only = !state.only; paint(); });
$("save").addEventListener("click", save);
$("close").addEventListener("click", () => {
  if (state.dirty && !confirm(t("Fechar sem salvar?", "Close without saving?"))) return;
  state.dirty = false;
  window.close();
  setTimeout(() => { location.href = "/"; }, 150);
});
window.addEventListener("beforeunload", (event) => { if (state.dirty) { event.preventDefault(); event.returnValue = ""; } });

async function load() {
  try {
    const data = await (await fetch("/api/layout", { cache: "no-store" })).json();
    if (typeof data.portuguese === "boolean") pt = data.portuguese;
    state.home = data.home || null;
    state.connected = new Set(data.connected || []);
    state.layout = data.layout && Array.isArray(data.layout.monitores) ? data.layout : { monitores: [] };
    state.zoom = fitZoom(state.layout.monitores, $("canvas").clientWidth);
    $("zoom").value = state.zoom;
    texts();
    buildPanel();
    paint();
    if (!state.home) {
      state.message = t("Este computador não é o principal: o layout fica no computador principal.", "This is not the main computer: the layout lives on the main computer.");
      paint();
    }
  } catch (error) {
    $("feedback").textContent = String(error);
  }
}
texts();
load();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn tela(machine: &str, id: &str, x: i32, y: i32) -> Monitor {
        Monitor {
            id: format!("{machine}:{id}"),
            machine: machine.into(),
            x,
            y,
            width: 1920,
            height: 1080,
            scale_fator: 1.0,
        }
    }

    #[test]
    fn editor_move_os_outros_mas_nao_as_telas_desta_maquina() {
        let saved = ScreenLayout { monitores: vec![tela("casa", "1", 0, 0), tela("mac", "1", 1920, 0)] };
        let edited = ScreenLayout {
            monitores: vec![tela("casa", "1", 500, 500), tela("mac", "1", -1920, 100), tela("intruso", "1", 4000, 0)],
        };
        let merged = merge_edit(&saved, &edited, "casa");
        let at: Vec<(&str, i32, i32)> = merged.monitores.iter().map(|m| (m.id.as_str(), m.x, m.y)).collect();
        assert_eq!(at, [("casa:1", 0, 0), ("mac:1", -1920, 100)]);
        let removed = merge_edit(&saved, &ScreenLayout { monitores: vec![] }, "casa");
        assert_eq!(removed.monitores.len(), 1);
    }
}
