import test from 'node:test';
import assert from 'node:assert/strict';
import { validateLayout, overlapping, snap, resize, fitZoom, onlyConnected } from './layout.js';
const a = { id: 'a', machine: 'desktop', x: -100, y: -50, width: 100, height: 100, scale_fator: 1 };
const b = { ...a, id: 'b', x: 0, scale_fator: 2 };
test('README layout accepts adjacent screens and negative coordinates', () => assert.equal(validateLayout({ monitores: [a, b] }), ''));
test('overlap and invalid scale blocked; empty layout valid', () => {
  assert.equal(overlapping([a, { ...b, x: -1 }]).size, 2);
  assert.ok(validateLayout({ monitores: [{ ...a, scale_fator: 4.1 }] }));
  assert.equal(validateLayout({ monitores: [{ ...a, width: 2147483647, height: 2147483647 }] }), '');
  assert.ok(validateLayout({ monitores: [{ ...a, width: 2147483648 }] }));
  assert.ok(validateLayout({ monitores: [{ ...a, height: 2147483648 }] }));
  assert.ok(validateLayout({ monitores: [a, { ...b, id: 'a' }] }));
  assert.equal(validateLayout({ monitores: [] }), '');
});
test('snap joins adjacent edges', () => assert.equal(snap({ ...b, x: 8 }, [a], 10).x, 0));
test('tamanho na mesa preserva a resolução real do monitor', () => {
  const mac = { id: 'mac:3', machine: 'mac', x: 0, y: -1440, width: 2560, height: 1440, scale_fator: 1 };
  const grown = resize(mac, 1.34375);
  assert.deepEqual([grown.width, grown.height, grown.scale_fator], [3440, 1935, 1.34375]);
  const back = resize(grown, 1);
  assert.deepEqual([back.width, back.height], [2560, 1440]);
  assert.equal(resize(mac, 9).scale_fator, 4, 'escala limitada a 4');
});
test('zoom cabe o layout inteiro no espaço disponível', () => {
  const wide = [{ ...a, x: -3390, width: 1920 }, { ...a, id: 'h', x: 0, width: 6000 }];
  const zoom = fitZoom(wide, 960);
  assert.ok(zoom > 0 && (6000 + 3390) * zoom <= 960);
  assert.equal(fitZoom([], 960), 0.17);
});
test('zoom nunca fica negativo em janela pequena', () => {
  assert.ok(fitZoom([{ ...a, width: 6000 }], 300) >= 0.03);
});
test('telas deste computador precisam se encostar: o Windows não aceita telas soltas', () => {
  const ultra = { id: 'pc:5', machine: 'pc', x: 0, y: 0, width: 3440, height: 1440, scale_fator: 1 };
  const note = { ...ultra, id: 'pc:1', x: 3440, width: 2560, height: 1600 };
  const mac = { ...ultra, id: 'mac:3', machine: 'mac', x: 3440, y: -1440 };
  assert.equal(validateLayout({ monitores: [ultra, note, mac] }, 'pc'), '');
  assert.ok(validateLayout({ monitores: [ultra, { ...note, x: 3500 }, mac] }, 'pc'), 'vão entre as telas da casa');
  assert.ok(validateLayout({ monitores: [ultra, { ...note, x: 3440, y: 1440 }] }, 'pc'), 'encostar só no canto não conta');
  assert.equal(validateLayout({ monitores: [ultra, { ...mac, x: 9000 }] }, 'pc'), '', 'outras máquinas podem ficar soltas');
});
test('onlyConnected drops ghost screens and keeps home and connected ones', () => {
  const home = { ...a, id: 'pc:1', machine: 'pc' }, mac = { ...b, id: 'mac:1', machine: 'mac' }, ghost = { ...b, id: 'mac:old', machine: 'mac', x: 500 };
  assert.deepEqual(onlyConnected([home, mac, ghost], ['mac:1'], 'pc').map(m => m.id), ['pc:1', 'mac:1']);
});
