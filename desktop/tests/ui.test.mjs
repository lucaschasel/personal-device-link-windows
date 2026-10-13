import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';
const source = await readFile(new URL('../src/ui.ts', import.meta.url), 'utf8');
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { escape, addressInput, filterTransfers, bluetoothInputError, transferEmptyMessage } = await import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'));
test('peer and filename HTML cannot create markup', () => {
  assert.equal(escape('<img src=x onerror="alert(1)"> & 中文'), '&lt;img src=x onerror=&quot;alert(1)&quot;&gt; &amp; 中文');
});
test('manual IP adds default port without rewriting explicit ports or IPv6', () => {
  assert.equal(addressInput(' 192.168.1.2 '), '192.168.1.2:45832');
  assert.equal(addressInput('192.168.1.2:40000'), '192.168.1.2:40000');
  assert.equal(addressInput('[::1]:45832'), '[::1]:45832');
  assert.equal(addressInput('invalid'), 'invalid');
});
test('waiting approvals stay in active transfers; failures are finished', () => {
  const rows = ['pending','waiting','preparing','transferring','verifying','complete','failed','cancelled','rejected'].map(phase => ({phase}));
  assert.equal(filterTransfers(rows,'all').length, 9);
  assert.deepEqual(filterTransfers(rows,'active').map(r => r.phase), ['pending','waiting','preparing','transferring','verifying']);
  assert.deepEqual(filterTransfers(rows,'finished').map(r => r.phase), ['complete','failed','cancelled','rejected']);
  assert.equal(rows.length, 9);
});

test('Bluetooth validates address and channel before starting the native helper', () => {
  assert.equal(bluetoothInputError('aa:bb:cc:dd:ee:ff', 7), '');
  assert.equal(bluetoothInputError('AA-BB-CC-DD-EE-FF', 30), '');
  for (const [address, channel] of [['', 1], ['$(whoami)', 1], ['AABBCCDDEEFF', 0], ['AABBCCDDEEFF', 31], ['AABBCCDDEEFF', 1.5], ['AABBCCDDEEFF', NaN]]) {
    assert.notEqual(bluetoothInputError(address, channel), '');
  }
});
test('Empty filter results distinguish active, finished and all transfers', () => {
  assert.equal(transferEmptyMessage('active')[0], '没有进行中的传输');
  assert.equal(transferEmptyMessage('finished')[0], '没有已结束的传输');
  assert.equal(transferEmptyMessage('all')[0], '还没有传输记录');
});
