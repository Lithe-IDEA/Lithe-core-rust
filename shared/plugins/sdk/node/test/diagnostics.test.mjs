import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createBackend, PluginError } from '../src/index.mjs';
import { TestHost } from './harness.mjs';

test('typed diagnostics validate facts, correlate the host operation, and suppress only successful polls', async (t) => {
  const backend = createBackend().function('work.run', ({ fail }, context) => {
    context.diagnostics.degraded('work.run', 'cache_unavailable', 2);
    context.diagnostics.recovered('work.run', 'cache_restored');
    if (fail) throw new PluginError('work.failed', 'Private user-facing explanation.');
    return null;
  }).suppressSuccessDiagnostics('work.run');
  const host = new TestHost(backend);
  t.after(() => host.close());
  const stderr = [];
  host.errorOutput.on('data', (chunk) => stderr.push(chunk.toString()));
  await host.initialize();
  for (const [operationID, fail] of [['host-success', false], ['host-failure', true]]) {
    await host.request('lithe/function/invoke', { functionID: 'work.run', operationID, params: { fail } });
  }
  const records = host.messages.filter(({ method }) => method === 'lithe/log').map(({ params }) => params);
  assert.deepEqual(records.map(({ kind }) => kind), ['degraded', 'recovered', 'degraded', 'recovered', 'operationFailed']);
  assert.equal(records.at(-1).operationId, 'host-failure');
  assert.equal(records.at(-1).code, 'work.failed');
  assert.ok(Number.isSafeInteger(records.at(-1).durationMs));
  assert.equal(stderr.join(''), '');
  assert.ok(!JSON.stringify(records).includes('Private user-facing'));
  for (const change of [
    () => backend.diagnostics.degraded('unknown.input', 'failure'),
    () => backend.diagnostics.degraded('work.run', 'body with spaces'),
    () => backend.diagnostics.degraded('work.run', 'failure', Number.MAX_SAFE_INTEGER + 1),
  ]) assert.throws(change);
});

test('the SDK requires litheLogging version 0 and never silently falls back to stderr', async (t) => {
  const host = new TestHost(createBackend());
  t.after(() => host.close());
  const reply = await host.request('initialize', { capabilities: {} });
  assert.equal(reply.error.code, -32600);
  assert.match(reply.error.message, /litheLogging version 0/);
  assert.equal(host.messages.filter(({ method }) => method === 'lithe/log').length, 0);
});

test('per-call suppression keeps user actions and failures', async (t) => {
  const backend = createBackend().function('database.call', ({ readOnly, fail }, context) => {
    if (readOnly) context.suppressSuccessDiagnostics();
    if (fail) throw new Error('private failure');
    return null;
  });
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize();
  for (const [id, params] of [['read', {readOnly:true}], ['query', {}], ['failure', {readOnly:true,fail:true}]]) {
    await host.request('lithe/function/invoke', {functionID:'database.call',operationID:id,params});
  }
  const facts = host.messages.filter(({method}) => method === 'lithe/log').map(({params}) => params);
  assert.deepEqual(facts.map(({kind,operationId}) => [kind,operationId]), [['operationFinished','query'],['operationFailed','failure']]);
});

test('initialize diagnostics wait for the capability answer and keep only eight facts', async (t) => {
  const backend = createBackend();
  backend.onInitialize(() => {
    for (let count = 0; count < 10; count++) backend.diagnostics.degraded('initialize', 'snapshot_unavailable', count);
  });
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize();
  assert.equal(host.messages[0].result.capabilities.experimental.litheLogging.version, 0);
  const facts = host.messages.filter(({method}) => method === 'lithe/log');
  assert.deepEqual(facts.map(({params}) => params.count), [0, 1, 2, 3, 4, 5, 6, 7]);
});

test('domain diagnostics reuse the original RPC id when no wire operation is present', async (t) => {
  const backend = createBackend();
  backend.build.model((_params, context) => {
    context.diagnostics.degraded('lithe/build/model', 'snapshot_unavailable');
    return { projects: [] };
  });
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize();
  await host.request('lithe/build/model', { root: '.' }, 42);
  await host.request('lithe/build/model', { root: '.', operationID: 'wire-original' }, 'rpc-second');
  const facts = host.messages.filter(({method}) => method === 'lithe/log').map(({params}) => params);
  assert.deepEqual(facts.map(({operationId}) => operationId), ['42', '42', 'wire-original', 'wire-original']);
});
