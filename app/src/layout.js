import { t } from './i18n.js';
export function validateLayout({ monitores }) {
  const ids = new Set();
  for (const m of monitores) {
    if (!m.id.trim() || ids.has(m.id) || !m.machine.trim()) return t('Informe máquina e IDs únicos, não vazios.', 'Every screen needs a computer and a unique, non-empty ID.');
    ids.add(m.id);
    if (![m.x, m.y, m.width, m.height].every(Number.isInteger) || m.x < -2147483648 || m.y < -2147483648 || m.x > 2147483647 || m.y > 2147483647 || m.width <= 0 || m.height <= 0 || m.width > 2147483647 || m.height > 2147483647 || !Number.isFinite(m.scale_fator) || m.scale_fator < .5 || m.scale_fator > 4) return t('Use dimensões inteiras positivas e escala entre 0,5 e 4.', 'Use positive whole-number sizes and a scale between 0.5 and 4.');
  }
  return overlapping(monitores).size ? t('Monitores sobrepostos. Separe as telas antes de salvar.', 'Screens overlap. Move them apart before saving.') : '';
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
