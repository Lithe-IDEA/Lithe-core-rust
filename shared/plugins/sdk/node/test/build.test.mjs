import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createBackend, LIMITS, PluginError } from '../src/index.mjs';
import { gate, TestHost, withTimeout } from './harness.mjs';

const launch = {
  executable: { kind: 'toolchain', value: 'project-jdk' },
  arguments: ['Main', 'a b'], workingDirectory: 'app', environment: {},
};
const methods = [
  ['execute', { exitCode: 0, output: 'built\n' }],
  ['test', { exitCode: 2, output: 'test failed\n' }],
  ['discoverTests', { tests: [{ suite: 'Suite', case: 'exact # [case]' }] }],
  ['prepareRun', launch],
  ['prepareDebug', { program: 'app/bin/main', arguments: [], workingDirectory: 'app', environment: {} }],
];

test('the build protocol answers version 0 and routes each of its typed methods', { timeout: 15000 }, async (t) => {
  const backend = createBackend();
  for (const [method, reply] of methods) backend.build[method](() => reply);
  const host = new TestHost(backend);
  t.after(() => host.close());
  const { result } = await host.initialize({ litheBuild: { versions: [0, 99] } });
  assert.equal(result.capabilities.experimental.litheBuild.version, 0);
  assert.equal(backend.buildVersion, 0);
  for (const [method, reply] of methods) {
    const response = await host.request(`lithe/build/${method}`, { operationID: `wire-${method}` });
    assert.deepEqual(response.result, method === 'discoverTests'
      ? { ...reply, format: 'litheTestDiscovery', version: 0 } : reply);
  }
});

test('build negotiation needs the host to offer version 0', { timeout: 15000 }, async (t) => {
  for (const versions of [[0], [99, 0], [1, 4], [99, '0', true], []]) {
    const backend = createBackend();
    backend.build.execute(() => ({ exitCode: 0, output: '' }));
    const host = new TestHost(backend);
    t.after(() => host.close());
    const offered = versions.includes(0);
    const response = await host.initialize({ litheBuild: { versions } });
    assert.equal(backend.buildVersion, offered ? 0 : null, JSON.stringify(versions));
    assert.deepEqual(response.result.capabilities.experimental,
      offered ? { litheLogging: { version: 0 }, litheBuild: { version: 0 } } : { litheLogging: { version: 0 } });
    await host.close();
  }
});

test('build progress preserves wire identity, UTF-8 limits and call ownership', { timeout: 15000 }, async (t) => {
  const release = gate();
  const entered = gate();
  let late;
  const backend = createBackend();
  backend.build.prepareRun(async (params, context) => {
    assert.equal(params.operationID, context.operationID);
    params.operationID = 'plugin-cannot-replace-the-wire-id';
    assert.equal(context.operationID, 'wire-42');
    late = context.progress;
    context.progress({ message: '😀'.repeat(2000), output: '界'.repeat(30000), percentage: 120 });
    entered.open();
    await release.opened;
    return launch;
  });
  const host = new TestHost(backend);
  t.after(async () => { release.open(); await host.close(); });
  await host.initialize({ litheBuild: { versions: [0] } });
  const pending = host.request('lithe/build/prepareRun', { operationID: 'wire-42' });
  await withTimeout(entered.opened, 'preparation to begin');
  const report = (await host.waitFor((message) => message.method === 'lithe/build/progress')).params;
  assert.equal(report.operationID, 'wire-42');
  assert.equal(Buffer.byteLength(report.message), 4096);
  assert.equal(Buffer.byteLength(report.output), 65535);
  assert.equal(report.percentage, 100);
  release.open();
  assert.deepEqual((await pending).result, launch);
  late({ message: 'late output', output: 'late' });
  assert.equal(host.messages.filter((message) => message.method === 'lithe/build/progress').length, 1);
});

test('native debug progress preserves wire identity, UTF-8 limits and call ownership', { timeout: 15000 }, async (t) => {
  const release = gate();
  const entered = gate();
  let late;
  const backend = createBackend();
  backend.build.prepareDebug(async (params, context) => {
    assert.equal(params.operationID, context.operationID);
    params.operationID = 'plugin-cannot-replace-the-wire-id';
    assert.equal(context.operationID, 'wire-42');
    late = context.progress;
    context.progress({ message: '😀'.repeat(2000), output: '界'.repeat(30000), percentage: 120 });
    entered.open();
    await release.opened;
    return methods.at(-1)[1];
  });
  const host = new TestHost(backend);
  t.after(async () => { release.open(); await host.close(); });
  await host.initialize({ litheBuild: { versions: [0] } });
  const pending = host.request('lithe/build/prepareDebug', { operationID: 'wire-42' });
  await withTimeout(entered.opened, 'preparation to begin');
  const report = (await host.waitFor((message) => message.method === 'lithe/build/progress')).params;
  assert.equal(report.operationID, 'wire-42');
  assert.equal(Buffer.byteLength(report.message), 4096);
  assert.equal(Buffer.byteLength(report.output), 65535);
  assert.equal(report.percentage, 100);
  release.open();
  assert.deepEqual((await pending).result, methods.at(-1)[1]);
  late({ message: 'late output', output: 'late' });
  assert.equal(host.messages.filter((message) => message.method === 'lithe/build/progress').length, 1);
});

test('cancelling one build drops its progress while a sibling call completes', { timeout: 15000 }, async (t) => {
  const entered = gate();
  const siblingEntered = gate();
  const release = gate();
  const backend = createBackend();
  backend.build.execute(async (_params, context) => {
    entered.open();
    await release.opened;
    context.progress({ message: 'cancelled progress' });
    return { exitCode: 0, output: '' };
  });
  backend.build.test(async () => {
    siblingEntered.open();
    await release.opened;
    return { exitCode: 3, output: 'independent failure' };
  });
  const host = new TestHost(backend);
  t.after(async () => { release.open(); await host.close(); });
  await host.initialize({ litheBuild: { versions: [0] } });
  const pending = host.request('lithe/build/execute', { operationID: 'wire-cancel' }, 42);
  const sibling = host.request('lithe/build/test', { operationID: 'wire-other' }, 43);
  await withTimeout(entered.opened, 'execution to begin');
  await withTimeout(siblingEntered.opened, 'sibling execution to begin');
  host.notify('$/cancelRequest', { id: 42 });
  release.open();
  assert.equal((await pending).error.code, -32800);
  assert.deepEqual((await sibling).result,
    { exitCode: 3, output: 'independent failure' });
  assert.ok(!host.messages.some((message) => message.method === 'lithe/build/progress'));
});

test('invalid build replies fail without terminating the session or falling back', { timeout: 15000 }, async (t) => {
  let reply;
  const backend = createBackend();
  backend.build.execute(() => reply);
  backend.build.discoverTests(() => ({ tests: [], format: 'wrong' }));
  backend.build.prepareRun(() => ({ ...launch, executable: false }));
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize({ litheBuild: { versions: [0] } });
  for (reply of [{}, { exitCode: true, output: '' }, { exitCode: 2147483648, output: '' },
    { exitCode: 0, output: false }, { exitCode: 0, output: 'x'.repeat(LIMITS.maxBuildResultBytes + 1) }]) {
    assert.equal((await host.request('lithe/build/execute', { operationID: 'wire-invalid' })).error.code, -32603);
  }
  for (const method of ['discoverTests', 'prepareRun']) {
    assert.equal((await host.request(`lithe/build/${method}`, { operationID: 'wire-invalid' })).error.code, -32603);
  }
  reply = { exitCode: 7, output: 'tool failure is a result' };
  assert.deepEqual((await host.request('lithe/build/execute', { operationID: 'wire-valid' })).result, reply);
});

test('build parameters need a wire operation ID and backend errors keep their category', { timeout: 15000 }, async (t) => {
  const backend = createBackend();
  backend.build.execute(() => { throw new PluginError('sample.failed', 'Build rejected'); });
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize({ litheBuild: { versions: [0] } });
  assert.equal((await host.request('lithe/build/execute', {})).error.code, -32602);
  assert.equal((await host.request('lithe/build/execute', [])).error.code, -32602);
  const failed = await host.request('lithe/build/execute', { operationID: 'wire-failed' });
  assert.deepEqual(failed.error, { code: -32000, message: 'Build rejected', data: { code: 'sample.failed' } });
});

test('native debug refuses adapter, process and arbitrary DAP result fields', { timeout: 15000 }, async (t) => {
  for (const field of ['adapter', 'launch', 'dapArguments']) {
    const backend = createBackend();
    backend.build.prepareDebug(() => ({ ...methods.at(-1)[1], [field]: 'untrusted' }));
    const host = new TestHost(backend);
    t.after(() => host.close());
    await host.initialize({ litheBuild: { versions: [0] } });
    assert.ok((await host.request('lithe/build/prepareDebug', { operationID: 'wire' })).error);
  }
});

test('read-only model inputs reach the model handler with the host identity', { timeout: 15000 }, async (t) => {
  const modelInputs = { version: 0, revision: 17, inputs: [
    { pluginID: 'dev.source', buildSystemID: 'source', state: 'unavailable', instances: [] },
  ] };
  const seen = [];
  const backend = createBackend();
  backend.build.model((params) => { seen.push(params.modelInputs); return { projects: [], targets: [], tasks: [] }; });
  const host = new TestHost(backend);
  t.after(() => host.close());
  await host.initialize({ litheBuild: { versions: [0] } });
  const response = await host.request('lithe/build/model', { modelInputs });
  assert.deepEqual(seen, [modelInputs]);
  assert.ok(response.result);
});
