import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createBackend } from '../src/index.mjs';
import { gate, TestHost, withTimeout } from './harness.mjs';

test('shortcut providers negotiate separately and read scoped snapshot inputs', async () => {
  const backend = createBackend({ name: 'test-backend' });
  backend.keymapImportProvider('first', async (snapshot) => {
    const files = await snapshot.listInputs();
    const bytes = await snapshot.readChunk(files[0].id, 0, 4);
    assert.equal(Buffer.from(bytes).toString('utf8'), 'data');
    return { bindings: [{ commandID: 'find-next', keys: [{ key: 'f3', modifiers: [] }] }] };
  });
  backend.keymapImportProvider('not-offered', () => ({ bindings: [] }));
  const host = new TestHost(backend);
  try {
    const { result } = await host.initialize({
      litheFunctions: { versions: [0] },
      litheKeymapImport: { versions: [0], providers: ['first', 'unknown'] },
    });
    assert.deepEqual(result.capabilities.experimental.litheKeymapImport, { version: 0, providers: ['first'] });
    const pending = host.request('lithe/keymap/import', { providerID: 'first', snapshotID: 'snapshot-1' });
    const list = await host.waitFor((message) => message.method === 'lithe/host/keymapImport/listInputs');
    host.send({ id: list.id, result: { files: [{ id: 'file-0', path: 'keys.json', byteCount: 4 }] } });
    const read = await host.waitFor((message) => message.method === 'lithe/host/keymapImport/readChunk');
    assert.equal(read.params.fileID, 'file-0');
    host.send({ id: read.id, result: { data: 'ZGF0YQ==' } });
    assert.deepEqual((await pending).result, { bindings: [{ commandID: 'find-next', keys: [{ key: 'f3', modifiers: [] }] }] });
    // The terminal fact precedes the response and names the wire method Core accepts.
    const facts = host.messages.filter((message) => message.method === 'lithe/log').map(({ params }) => [params.kind, params.operation]);
    assert.deepEqual(facts, [['operationFinished', 'lithe/keymap/import']]);
    assert.ok((await host.request('lithe/keymap/import', { providerID: 'not-offered', snapshotID: 'snapshot-1' })).error);
    assert.ok((await host.request('lithe/keymap/import', { providerID: 'first', snapshotID: '../private' })).error);
  } finally { await host.close(); }
});

test('cancellation reaches an active shortcut provider', async () => {
  const started = gate();
  const backend = createBackend().keymapImportProvider('first', async (_snapshot, context) => {
    started.open();
    await new Promise((resolve) => context.signal.addEventListener('abort', resolve, { once: true }));
    context.throwIfCancelled();
    return { bindings: [] };
  });
  const host = new TestHost(backend);
  try {
    await host.initialize({ litheKeymapImport: { versions: [0], providers: ['first'] } });
    const pending = host.request('lithe/keymap/import', { providerID: 'first', snapshotID: 'snapshot-1' }, 42);
    await withTimeout(started.opened, 'provider to start');
    host.notify('$/cancelRequest', { id: 42 });
    assert.equal((await pending).error.code, -32800);
  } finally { await host.close(); }
});
