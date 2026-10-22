// Idioma da interface: o do agente (perguntado ao lado nativo antes da primeira renderização);
// no navegador (prévia), o do próprio navegador. Português ou inglês.
let portuguese = typeof navigator !== 'undefined' && (navigator.language || '').toLowerCase().startsWith('pt');

export function setPortuguese(value) {
  portuguese = value;
  if (typeof document !== 'undefined') document.documentElement.lang = portuguese ? 'pt-BR' : 'en';
}

export const t = (pt, en) => (portuguese ? pt : en);
