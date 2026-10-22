import test from 'node:test';
import assert from 'node:assert/strict';
import { validateLayout, overlapping, snap, resize, fitZoom } from './layout.js';
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
