import { invoke, isTauri } from '@tauri-apps/api/core';
import { t } from './i18n.js';
// No navegador (prévia de desenvolvimento) não há agente: carregar devolve nulo e salvar só avisa.
export async function call(command, args = {}) {
  if (isTauri()) return invoke(command, args);
  if (command === 'save_layout') throw new Error(t('Prévia no navegador: salvar exige o editor aberto pelo ícone da bandeja.', 'Browser preview: saving needs the editor opened from the tray icon.'));
  return null;
}
