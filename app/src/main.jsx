import React, { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { call } from './bridge';
import { fitZoom, onlyConnected, overlapping, resize, screenNumbers, sharedEdges, snap, validateLayout } from './layout';
import { setPortuguese, t } from './i18n.js';
import icon from './icon.png';
import './index.css';

// Prévia no navegador (sem o agente): o layout real chega por `load_layout` no app nativo.
const preview = () => ({ monitores: [
  { id: 'casa:1', machine: t('Este computador', 'This computer'), x: 0, y: 0, width: 3440, height: 1440, scale_fator: 1 },
  { id: 'mac:1', machine: 'mac', x: -1470, y: 242, width: 1470, height: 956, scale_fator: 1 }
] });
// Cores das máquinas: --machine-0..5 em index.css (claras no escuro, escuras no claro).
const MACHINE_COLORS = 6;

function App() {
  const [doc, setDoc] = useState({ layout: null, saved: null, past: [], future: [] });
  const { layout } = doc;
  const [selected, select] = useState(null);
  const [message, notify] = useState('');
  const [identifyError, setIdentifyError] = useState('');
  const [zoom, setZoom] = useState(0.17);
  const [home, setHome] = useState(null);
  const [connected, setConnected] = useState(null);
  const drag = useRef(null), canvas = useRef(null), gesture = useRef(null);

  useEffect(() => {
    call('home_machine').then(name => setHome(name ?? null)).catch(() => setHome(null));
    call('connected_screens').then(ids => setConnected(ids ?? null)).catch(() => setConnected(null));
    call('load_layout').then(data => {
      const loaded = data ?? preview();
      setDoc({ layout: loaded, saved: loaded, past: [], future: [] });
      // Área do canvas = janela menos painel lateral e margens (o canvas ainda não existe neste momento).
      setZoom(fitZoom(loaded.monitores, window.innerWidth - 420));
    }).catch(e => notify(String(e)));
  }, []);

  // Atalhos de desfazer/refazer. Um gesto contínuo (arrasto, seta segurada, digitação) é um passo só.
  useEffect(() => {
    const endGesture = () => { gesture.current = null; };
    const onKeyUp = e => { if (e.key.startsWith('Arrow')) endGesture(); };
    const onKeyDown = e => {
      const typing = e.target instanceof HTMLInputElement && e.target.type !== 'range';
      if (!(e.ctrlKey || e.metaKey) || typing) return;
      const key = e.key.toLowerCase();
      if (key === 'z' && !e.shiftKey) { e.preventDefault(); undo(); }
      else if (key === 'y' || (key === 'z' && e.shiftKey)) { e.preventDefault(); redo(); }
    };
    window.addEventListener('pointerup', endGesture);
    window.addEventListener('keyup', onKeyUp);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerup', endGesture);
      window.removeEventListener('keyup', onKeyUp);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, []);

  if (!layout) return <div className="flex min-h-screen flex-col font-sans"><main><p className="feedback text-muted">{message || t('Carregando layout…', 'Loading layout…')}</p></main></div>;

  const machines = [...new Set(layout.monitores.map(m => m.machine))];
  const color = machine => `var(--machine-${machines.indexOf(machine) % MACHINE_COLORS})`;
  const conflict = overlapping(layout.monitores), error = validateLayout(layout, home);
  const active = layout.monitores.find(m => m.id === selected);
  const dirty = layout !== doc.saved;
  // Telas deste computador são reais: movem (o Windows é rearrumado ao salvar), mas não mudam de tamanho nem somem.
  const isHome = m => m.machine === home;
  // Mesma regra do servidor: só telas conectadas, de todas as máquinas, da esquerda para a direita.
  const numbers = screenNumbers(connected ? onlyConnected(layout.monitores, connected, home) : layout.monitores);
  const edges = sharedEdges(layout.monitores);
  const originX = 80 - Math.min(0, ...layout.monitores.map(m => m.x)) * zoom;
  const originY = 80 - Math.min(0, ...layout.monitores.map(m => m.y)) * zoom;
  const failure = error || identifyError;

  // Desfazer: cada edição guarda o layout anterior; gesto com a mesma chave vira um passo só.
  function edit(change, key) {
    const merge = key !== undefined && key === gesture.current;
    gesture.current = key ?? null;
    setDoc(d => ({ ...d, layout: change(d.layout), past: merge ? d.past : [...d.past, d.layout], future: [] }));
    setIdentifyError('');
    notify('');
  }
  function update(id, change, key) {
    edit(l => ({ monitores: l.monitores.map(m => m.id === id ? change(m, l.monitores) : m) }), key);
  }
  function undo() {
    gesture.current = null;
    setDoc(d => d.past.length ? { ...d, layout: d.past.at(-1), past: d.past.slice(0, -1), future: [d.layout, ...d.future] } : d);
  }
  function redo() {
    gesture.current = null;
    setDoc(d => d.future.length ? { ...d, layout: d.future[0], past: [...d.past, d.layout], future: d.future.slice(1) } : d);
  }
  async function save() {
    if (error) return notify(error);
    try {
      await call('save_layout', { layout });
      setDoc(d => ({ ...d, saved: layout }));
      notify(t('Salvo. Já vale para o cursor; teste e ajuste à vontade.', 'Saved. The cursor already uses it; test and keep adjusting.'));
    } catch (e) { notify(String(e)); }
  }
  function identify() {
    setIdentifyError('');
    call('identify_screens').catch(e => setIdentifyError(String(e)));
  }
  // Telas fantasmas (de máquinas ou monitores que não voltam) atrapalham o encaixe das reais.
  const ghosts = connected ? layout.monitores.length - onlyConnected(layout.monitores, connected, home).length : 0;
  function dropGhosts() {
    edit(l => ({ monitores: onlyConnected(l.monitores, connected, home) }));
    select(null);
    notify(t('Telas fantasmas removidas. Salve para aplicar.', 'Ghost screens removed. Save to apply.'));
  }

  return <div className="flex min-h-screen flex-col font-sans">
    <header className="flex h-[78px] items-center gap-7 border-b border-line px-9 max-[850px]:px-5">
      <span className="flex items-center gap-3 font-display text-[26px] font-extrabold text-brand"><img src={icon} alt="" width="30" height="30"/><span>MouseX</span></span>
      <span className="eyebrow max-[850px]:hidden">{t('ARRUMAR TELAS', 'ARRANGE SCREENS')}</span>
    </header>
    <main className="flex-1 px-9 py-[38px] max-[850px]:px-5 max-[850px]:py-[26px] min-[1500px]:mx-auto min-[1500px]:w-full min-[1500px]:max-w-[1600px]">
      <div className="mb-[26px] flex items-center justify-between gap-6 max-[850px]:items-start max-[560px]:flex-col">
        <div>
          <div className="eyebrow">{t('LAYOUT DA SUA MESA', 'YOUR DESK LAYOUT')}</div>
          <h1 className="mb-3 mt-[9px] font-display text-[32px] font-bold tracking-[-1.3px] max-[850px]:text-[27px]">{t('Arrume as telas como estão na mesa', 'Arrange the screens as they sit on your desk')}</h1>
          <p className="mb-0 max-w-[660px]">{t('Arraste cada tela. O cursor passa entre telas cujas bordas se encostam.', 'Drag each screen. The cursor moves between screens whose edges touch.')}</p>
        </div>
        <div className="flex items-center gap-2.5">
          <button className="btn" onClick={() => call('close_editor')}>{dirty ? t('Fechar sem salvar', 'Close without saving') : t('Fechar', 'Close')}</button>
          <button className="btn" onClick={identify} title={t('Mostra o número de cada tela nos monitores deste computador', 'Shows each screen’s number on this computer’s monitors')}>{t('Identificar telas', 'Identify screens')}</button>
          <button className="btn" onClick={dropGhosts} disabled={!ghosts} title={t('Remove as telas que não estão conectadas agora', 'Removes screens that are not connected right now')}>{t('Só telas conectadas', 'Connected screens only')}{ghosts ? ` (${ghosts})` : ''}</button>
          <button className="btn btn-primary" onClick={save} disabled={!!error || !dirty}>{t('Salvar', 'Save')}</button>
        </div>
      </div>
      <div className="grid grid-cols-[minmax(0,1fr)_258px] overflow-hidden rounded-[10px] border border-line bg-panel max-[850px]:grid-cols-1">
        <section className="min-w-0" aria-label={t('Telas', 'Screens')}>
          <div className="flex items-center justify-between border-b border-line px-[18px] py-4 text-xs text-soft"><span>{machines.map(m => <span key={m} className="mr-4 inline-flex items-center gap-1.5 text-muted"><i className="inline-block size-2.5 rounded-[3px]" style={{ background: color(m) }}/>{m}</span>)}</span></div>
          <div className="relative h-[430px] overflow-auto bg-canvas [background-image:radial-gradient(var(--dot)_0.8px,transparent_0.8px)] [background-size:18px_18px] min-[1500px]:h-[520px] max-[850px]:h-[360px]" ref={canvas} onPointerMove={e => {
            if (!drag.current) return;
            const d = drag.current;
            update(d.id, (m, all) => snap({ ...m, x: Math.round(d.x + (e.clientX - d.px) / zoom), y: Math.round(d.y + (e.clientY - d.py) / zoom) }, all.filter(o => o.id !== d.id), 12 / zoom), `drag:${d.id}`);
          }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
            <div className="relative min-h-full min-w-full" style={{ width: Math.max(900, ...layout.monitores.map(m => (m.x + m.width) * zoom + originX + 80)), height: Math.max(420, ...layout.monitores.map(m => (m.y + m.height) * zoom + originY + 80)) }}>
              {layout.monitores.map(m => <button key={m.id}
                className={`absolute flex min-h-[18px] min-w-[18px] touch-none select-none flex-col items-center justify-center gap-2 overflow-hidden rounded-lg p-2 shadow-[0_8px_16px_#0003] ${selected === m.id ? 'border-2 shadow-[0_0_0_4px_rgb(158_232_195/0.05)]' : 'border'} ${conflict.has(m.id) ? 'bg-conflict' : selected === m.id ? 'bg-tile-selected' : 'bg-tile'}`}
                style={{ left: originX + m.x * zoom, top: originY + m.y * zoom, width: m.width * zoom, height: m.height * zoom, borderColor: color(m.machine) }}
                onPointerDown={e => { if (e.button !== 0) return; select(m.id); e.currentTarget.setPointerCapture(e.pointerId); drag.current = { id: m.id, x: m.x, y: m.y, px: e.clientX, py: e.clientY }; }}
                onKeyDown={e => { const dir = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key]; if (dir) { e.preventDefault(); const step = e.shiftKey ? 10 : 1; update(m.id, o => ({ ...o, x: o.x + dir[0] * step, y: o.y + dir[1] * step }), `nudge:${m.id}`); } }}
                aria-label={`${m.machine}, ${Math.round(m.width / m.scale_fator)} ${t('por', 'by')} ${Math.round(m.height / m.scale_fator)}${conflict.has(m.id) ? t(', sobreposta', ', overlapping') : ''}`}>
                {numbers.has(m.id) && <span className={`text-[23px] font-medium ${selected === m.id ? 'text-accent-ink' : 'text-number'}`}>{numbers.get(m.id)}</span>}
                <strong className="max-w-full truncate text-[13px] font-bold" style={{ color: color(m.machine) }}>{m.machine}</strong>
                <span className="text-[10px] text-tile-text">{Math.round(m.width / m.scale_fator)} × {Math.round(m.height / m.scale_fator)}</span>
                {conflict.has(m.id) && <b className="text-[10px] font-bold text-conflict-ink">{t('Sobreposta', 'Overlapping')}</b>}
              </button>)}
              <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible" aria-hidden="true">
                {edges.map(e => <line key={`${e.a}|${e.b}`} className="stroke-accent-ink" strokeWidth="3" strokeLinecap="round" x1={originX + e.x1 * zoom} y1={originY + e.y1 * zoom} x2={originX + e.x2 * zoom} y2={originY + e.y2 * zoom}/>)}
              </svg>
            </div>
          </div>
          <div className="flex items-center justify-between gap-2.5 border-t border-line px-4 py-[13px] text-[11px] text-muted max-[850px]:flex-wrap"><span className="max-[560px]:max-w-[220px]">{t('Arraste para mover · Setas: 1 px · Shift + setas: 10 px', 'Drag to move · Arrows: 1 px · Shift + arrows: 10 px')}</span>
            <label className="flex items-center gap-2 text-label">Zoom <input className="field mt-[7px]" type="range" min="0.03" max="0.3" step="0.01" value={zoom} onChange={e => setZoom(+e.target.value)}/></label></div>
        </section>
        <aside className="border-l border-line px-5 py-[22px] max-[850px]:border-l-0 max-[850px]:border-t">
          <div className="eyebrow">{t('TELA SELECIONADA', 'SELECTED SCREEN')}</div>
          {active ? <>
            <h2 className="mb-[22px] font-display text-[19px] tracking-[-0.5px] [overflow-wrap:anywhere]" style={{ color: color(active.machine) }}>{active.machine}</h2>
            <p className="hint">{t('Resolução', 'Resolution')} {Math.round(active.width / active.scale_fator)} × {Math.round(active.height / active.scale_fator)} ({active.id})</p>
            {isHome(active) && <p className="hint">{t('Tela deste computador: ao salvar, o Windows passa a usar esta mesma arrumação.', 'One of this computer’s screens: when you save, Windows is rearranged to match.')}</p>}
            {!isHome(active) && <label className="field-label mb-[15px]">{t('Tamanho na mesa', 'Size on desk')}
              <input className="field mt-[7px]" type="range" min="0.5" max="4" step="0.01" value={active.scale_fator} onChange={e => update(active.id, m => resize(m, +e.target.value), `resize:${active.id}`)}/>
              <span className="hint">{Math.round(active.scale_fator * 100)}% · {t('aumente para alinhar com telas fisicamente maiores', 'increase it to match physically larger screens')}</span>
            </label>}
            <div className="grid grid-cols-2 gap-x-3 max-[850px]:grid-cols-4 max-[560px]:grid-cols-2">{[['x', t('Posição X', 'Position X')], ['y', t('Posição Y', 'Position Y')]].map(([key, label]) =>
              <label key={key} className="field-label mb-[15px]">{label}<input className="field mt-[7px]" type="number" step="1" value={active[key]} onChange={e => { const v = Math.round(Number(e.target.value) || 0); update(active.id, m => ({ ...m, [key]: v }), `field:${active.id}:${key}`); }}/></label>)}</div>
            {!isHome(active) && <><button className="btn btn-danger" onClick={() => { edit(l => ({ monitores: l.monitores.filter(m => m.id !== active.id) })); select(null); }}>{t('Remover tela', 'Remove screen')}</button>
            <p className="hint">{t('Tela removida volta sozinha quando a máquina reconectar.', 'A removed screen comes back when its computer reconnects.')}</p></>}
          </> : <p>{t('Clique numa tela para ajustar.', 'Click a screen to adjust it.')}</p>}
        </aside>
      </div>
      <div className={`feedback ${failure ? 'text-error' : 'text-muted'}`} role="status">{failure || message || (dirty ? t('Alterações não salvas.', 'Unsaved changes.') : t('Bordas se encaixam sozinhas ao arrastar.', 'Edges snap together while you drag.'))}</div>
    </main>
  </div>;
}
// Idioma antes da primeira renderização: os textos já saem certos.
call('portuguese')
  .then(value => { if (typeof value === 'boolean') setPortuguese(value); })
  .catch(() => {})
  .finally(() => createRoot(document.getElementById('root')).render(<App/>));
