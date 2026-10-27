import { t } from './i18n.js';
// Encostam quando dividem um trecho de borda (só o canto não basta): é por ali que o cursor passa.
// Devolve o trecho compartilhado em coordenadas do layout, ou null. Mesma regra do `touching` do Rust.
const sharedEdge = (a, b) => {
  const top = Math.max(a.y, b.y), bottom = Math.min(a.y + a.height, b.y + b.height);
  const left = Math.max(a.x, b.x), right = Math.min(a.x + a.width, b.x + b.width);
  if (top < bottom && (a.x + a.width === b.x || b.x + b.width === a.x)) {
    const x = a.x + a.width === b.x ? b.x : a.x;
    return { x1: x, y1: top, x2: x, y2: bottom };
  }
  if (left < right && (a.y + a.height === b.y || b.y + b.height === a.y)) {
    const y = a.y + a.height === b.y ? b.y : a.y;
    return { x1: left, y1: y, x2: right, y2: y };
  }
  return null;
};
const touching = (a, b) => sharedEdge(a, b) !== null;
// Trechos de borda entre pares de telas, para destacar no canvas.
export function sharedEdges(monitores) {
  const out = [];
  for (let i = 0; i < monitores.length; i++) for (let j = i + 1; j < monitores.length; j++) {
    const edge = sharedEdge(monitores[i], monitores[j]);
    if (edge) out.push({ a: monitores[i].id, b: monitores[j].id, ...edge });
  }
  return out;
}
// Número de cada tela da esquerda para a direita (desempate por y). O Identificar telas usa a mesma regra.
export function screenNumbers(monitores) {
  const ordered = [...monitores].sort((a, b) => a.x - b.x || a.y - b.y);
  return new Map(ordered.map((m, i) => [m.id, i + 1]));
}
// Telas de uma máquina formam um bloco só? O Windows não aceita telas soltas na área de trabalho.
export function connected(monitores) {
  const reached = new Set(monitores.slice(0, 1));
  for (const m of reached) for (const o of monitores) if (!reached.has(o) && touching(m, o)) reached.add(o);
  return reached.size === monitores.length;
}
export function validateLayout({ monitores }, home) {
  const ids = new Set();
  for (const m of monitores) {
    if (!m.id.trim() || ids.has(m.id) || !m.machine.trim()) return t('Informe máquina e IDs únicos, não vazios.', 'Every screen needs a computer and a unique, non-empty ID.');
    ids.add(m.id);
    if (![m.x, m.y, m.width, m.height].every(Number.isInteger) || m.x < -2147483648 || m.y < -2147483648 || m.x > 2147483647 || m.y > 2147483647 || m.width <= 0 || m.height <= 0 || m.width > 2147483647 || m.height > 2147483647 || !Number.isFinite(m.scale_fator) || m.scale_fator < .5 || m.scale_fator > 4) return t('Use dimensões inteiras positivas e escala entre 0,5 e 4.', 'Use positive whole-number sizes and a scale between 0.5 and 4.');
  }
  if (overlapping(monitores).size) return t('Monitores sobrepostos. Separe as telas antes de salvar.', 'Screens overlap. Move them apart before saving.');
  return home && !connected(monitores.filter(m => m.machine === home)) ? t('As telas deste computador precisam se encostar: o Windows não aceita telas soltas.', 'This computer’s screens must touch each other: Windows does not allow detached screens.') : '';
}
export function overlapping(monitores) {
  const ids = new Set();
  for (let i = 0; i < monitores.length; i++) for (let j = i + 1; j < monitores.length; j++) {
    const a = monitores[i], b = monitores[j];
    if (a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y) { ids.add(a.id); ids.add(b.id); }
  }
  return ids;
}
export function snap(m, others, threshold = 60) {
  let x = m.x, y = m.y, dx = threshold, dy = threshold;
  for (const b of others) {
    for (const edge of [b.x, b.x + b.width, b.x - m.width, b.x + b.width - m.width]) if (Math.abs(edge - m.x) < dx) { dx = Math.abs(edge - m.x); x = edge; }
    for (const edge of [b.y, b.y + b.height, b.y - m.height, b.y + b.height - m.height]) if (Math.abs(edge - m.y) < dy) { dy = Math.abs(edge - m.y); y = edge; }
  }
  return { ...m, x, y };
}
// Tamanho na mesa: muda a área ocupada no layout sem mudar a resolução real (largura/escala é constante).
export function resize(m, scale) {
  const s = Math.min(4, Math.max(0.5, scale));
  const nativeW = m.width / m.scale_fator, nativeH = m.height / m.scale_fator;
  return { ...m, width: Math.round(nativeW * s), height: Math.round(nativeH * s), scale_fator: s };
}
// Zoom que cabe todo o layout na largura disponível (com margem), limitado ao padrão de 17%.
export function fitZoom(monitores, available) {
  if (!monitores.length) return 0.17;
  const left = Math.min(...monitores.map(m => m.x)), right = Math.max(...monitores.map(m => m.x + m.width));
  return Math.max(0.03, Math.min(0.17, (available - 160) / (right - left)));
}
// Só telas conectadas agora: fantasmas (máquina ou tela que não volta) somem; as presentes ficam onde estão.
export function onlyConnected(monitores, connected, home) {
  const ids = new Set(connected);
  return monitores.filter(m => m.machine === home || ids.has(m.id));
}
