import React, { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { call } from './bridge';
import { fitZoom, overlapping, resize, snap, validateLayout } from './layout';
import { setPortuguese, t } from './i18n.js';
import icon from './icon.png';
import './style.css';

// Prévia no navegador (sem o agente): o layout real chega por `load_layout` no app nativo.
const preview = () => ({ monitores: [
  { id: 'casa:1', machine: t('Este computador', 'This computer'), x: 0, y: 0, width: 3440, height: 1440, scale_fator: 1 },
  { id: 'mac:1', machine: 'mac', x: -1470, y: 242, width: 1470, height: 956, scale_fator: 1 }
] });
const palette = ['#9ee8c3', '#8ec5ff', '#ffc98e', '#e7a1ff', '#ff9e9e', '#d8e88e'];

function App() {
  const [layout, setLayout] = useState(null);
  const [selected, select] = useState(null);
  const [message, notify] = useState('');
  const [dirty, setDirty] = useState(false);
  const [zoom, setZoom] = useState(0.17);
  const [home, setHome] = useState(null);
  const drag = useRef(null), canvas = useRef(null);

  useEffect(() => {
    call('home_machine').then(name => setHome(name ?? null)).catch(() => setHome(null));
    call('load_layout').then(data => {
      const loaded = data ?? preview();
      setLayout(loaded);
      // Área do canvas = janela menos painel lateral e margens (o canvas ainda não existe neste momento).
      setZoom(fitZoom(loaded.monitores, window.innerWidth - 420));
    }).catch(e => notify(String(e)));
  }, []);

  if (!layout) return <div className="shell"><main><p className="feedback">{message || t('Carregando layout…', 'Loading layout…')}</p></main></div>;

  const machines = [...new Set(layout.monitores.map(m => m.machine))];
  const color = machine => palette[machines.indexOf(machine) % palette.length];
  const conflict = overlapping(layout.monitores), error = validateLayout(layout, home);
  const active = layout.monitores.find(m => m.id === selected);
  // Telas deste computador são reais: movem (o Windows é rearrumado ao salvar), mas não mudam de tamanho nem somem.
  const isHome = m => m.machine === home;
  const originX = 80 - Math.min(0, ...layout.monitores.map(m => m.x)) * zoom;
  const originY = 80 - Math.min(0, ...layout.monitores.map(m => m.y)) * zoom;

  function update(id, change) {
    setLayout(l => ({ monitores: l.monitores.map(m => m.id === id ? change(m) : m) }));
    setDirty(true);
    notify('');
  }
  async function save() {
    if (error) return notify(error);
    try { await call('save_layout', { layout }); } catch (e) { notify(String(e)); }
  }

  return <div className="shell">
    <header><span className="brand"><img src={icon} alt="" width="30" height="30"/><span>MouseX</span></span><span className="tag">{t('ARRUMAR TELAS', 'ARRANGE SCREENS')}</span></header>
    <main>
      <div className="page-title">
        <div><div className="eyebrow">{t('LAYOUT DA SUA MESA', 'YOUR DESK LAYOUT')}</div><h1>{t('Arrume as telas como estão na mesa', 'Arrange the screens as they sit on your desk')}</h1><p>{t('Arraste cada tela. O cursor passa entre telas cujas bordas se encostam.', 'Drag each screen. The cursor moves between screens whose edges touch.')}</p></div>
        <div className="actions">
          <button onClick={() => call('close_editor')}>{dirty ? t('Fechar sem salvar', 'Close without saving') : t('Fechar', 'Close')}</button>
          <button className="primary" onClick={save} disabled={!!error}>{t('Salvar e fechar', 'Save and close')}</button>
        </div>
      </div>
      <div className="workspace">
        <section className="editor" aria-label={t('Telas', 'Screens')}>
          <div className="toolbar"><span>{machines.map(m => <span key={m} className="legend"><i style={{ background: color(m) }}/>{m}</span>)}</span></div>
          <div className="canvas" ref={canvas} onPointerMove={e => {
            if (!drag.current) return;
            const d = drag.current;
            update(d.id, m => snap({ ...m, x: Math.round(d.x + (e.clientX - d.px) / zoom), y: Math.round(d.y + (e.clientY - d.py) / zoom) }, layout.monitores.filter(o => o.id !== d.id), 12 / zoom));
          }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
            <div className="canvas-world" style={{ width: Math.max(900, ...layout.monitores.map(m => (m.x + m.width) * zoom + originX + 80)), height: Math.max(420, ...layout.monitores.map(m => (m.y + m.height) * zoom + originY + 80)) }}>
              {layout.monitores.map(m => <button key={m.id}
                className={`monitor ${selected === m.id ? 'selected' : ''} ${conflict.has(m.id) ? 'conflict' : ''}`}
                style={{ left: originX + m.x * zoom, top: originY + m.y * zoom, width: m.width * zoom, height: m.height * zoom, borderColor: color(m.machine) }}
                onPointerDown={e => { if (e.button !== 0) return; select(m.id); e.currentTarget.setPointerCapture(e.pointerId); drag.current = { id: m.id, x: m.x, y: m.y, px: e.clientX, py: e.clientY }; }}
                onKeyDown={e => { const dir = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key]; if (dir) { e.preventDefault(); const step = e.shiftKey ? 10 : 1; update(m.id, o => ({ ...o, x: o.x + dir[0] * step, y: o.y + dir[1] * step })); } }}
                aria-label={`${m.machine}, ${Math.round(m.width / m.scale_fator)} ${t('por', 'by')} ${Math.round(m.height / m.scale_fator)}${conflict.has(m.id) ? t(', sobreposta', ', overlapping') : ''}`}>
                <strong style={{ color: color(m.machine) }}>{m.machine}</strong>
                <span>{Math.round(m.width / m.scale_fator)} × {Math.round(m.height / m.scale_fator)}</span>
                {conflict.has(m.id) && <b>{t('Sobreposta', 'Overlapping')}</b>}
              </button>)}
            </div>
          </div>
          <div className="canvas-footer"><span>{t('Arraste para mover · Setas: 1 px · Shift + setas: 10 px', 'Drag to move · Arrows: 1 px · Shift + arrows: 10 px')}</span>
            <label>Zoom <input type="range" min="0.03" max="0.3" step="0.01" value={zoom} onChange={e => setZoom(+e.target.value)}/></label></div>
        </section>
        <aside>
          <div className="eyebrow">{t('TELA SELECIONADA', 'SELECTED SCREEN')}</div>
          {active ? <>
            <h2 style={{ color: color(active.machine) }}>{active.machine}</h2>
            <p className="hint">{t('Resolução', 'Resolution')} {Math.round(active.width / active.scale_fator)} × {Math.round(active.height / active.scale_fator)} ({active.id})</p>
            {isHome(active) && <p className="hint">{t('Tela deste computador: ao salvar, o Windows passa a usar esta mesma arrumação.', 'One of this computer’s screens: when you save, Windows is rearranged to match.')}</p>}
            {!isHome(active) && <label>{t('Tamanho na mesa', 'Size on desk')}
              <input type="range" min="0.5" max="4" step="0.01" value={active.scale_fator} onChange={e => update(active.id, m => resize(m, +e.target.value))}/>
              <span className="hint">{Math.round(active.scale_fator * 100)}% · {t('aumente para alinhar com telas fisicamente maiores', 'increase it to match physically larger screens')}</span>
            </label>}
            <div className="fields">{[['x', t('Posição X', 'Position X')], ['y', t('Posição Y', 'Position Y')]].map(([key, label]) =>
              <label key={key}>{label}<input type="number" step="1" value={active[key]} onChange={e => update(active.id, m => ({ ...m, [key]: Math.round(Number(e.target.value) || 0) }))}/></label>)}</div>
            {!isHome(active) && <><button className="danger" onClick={() => { setLayout(l => ({ monitores: l.monitores.filter(m => m.id !== active.id) })); select(null); setDirty(true); }}>{t('Remover tela', 'Remove screen')}</button>
            <p className="hint">{t('Tela removida volta sozinha quando a máquina reconectar.', 'A removed screen comes back when its computer reconnects.')}</p></>}
          </> : <p>{t('Clique numa tela para ajustar.', 'Click a screen to adjust it.')}</p>}
        </aside>
      </div>
      <div className={error ? 'feedback error' : 'feedback'} role="status">{error || message || (dirty ? t('Alterações não salvas.', 'Unsaved changes.') : t('Bordas se encaixam sozinhas ao arrastar.', 'Edges snap together while you drag.'))}</div>
    </main>
  </div>;
}
// Idioma antes da primeira renderização: os textos já saem certos.
call('portuguese')
  .then(value => { if (typeof value === 'boolean') setPortuguese(value); })
  .catch(() => {})
  .finally(() => createRoot(document.getElementById('root')).render(<App/>));
