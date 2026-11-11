import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createBackend, HOST_ERRORS, HostError, LIMITS, PluginSdkError } from '../src/index.mjs';
import { gate, TestHost, withTimeout } from './harness.mjs';

const fixture = JSON.parse(
  readFileSync(new URL('../../../contract/v1/fixtures/backend/function-protocol.json', import.meta.url), 'utf8'),
);

const CONTEXT = Object.freeze({ contextID: 'w1', generation: 2 });

/** A backend and host whose session negotiated plugin functions. */
async function session(backend = createBackend()) {
  const host = new TestHost(backend);
  await host.initialize({ litheFunctions: { versions: [0] } });
  return { backend, host };
}

/** The next host request the backend wrote that the test has not taken yet. */
function nextRequest(host, method, taken = new Set()) {
  return host
    .waitFor((message) => message.method === method && message.id !== undefined && !taken.has(message.id), method)
    .then((message) => {
      taken.add(message.id);
      return message;
    });
}

test('the SDK answers litheFunctions only when the host offers version 0', async () => {
  const backend = createBackend().function('query.run', () => null);
  const host = new TestHost(backend);
  const { result } = await host.initialize({ litheFunctions: { versions: [0] } });
  assert.deepEqual(result.capabilities.experimental.litheFunctions, { version: 0, functions: [{ id: 'query.run' }] });
  assert.equal(backend.functionsVersion, 0);
  assert.equal(backend.host.negotiated, true);
  await host.close();

  for (const versions of [[1], [99], []]) {
    const other = createBackend().function('query.run', () => null);
    const otherHost = new TestHost(other);
    const answer = await otherHost.initialize({ litheFunctions: { versions } });
    assert.equal(answer.result.capabilities.experimental.litheFunctions, undefined, JSON.stringify(versions));
    assert.equal(other.functionsVersion, null);
    assert.equal(other.host.negotiated, false);
    await otherHost.close();
  }
});

test('credentials round-trip through the host, with the contract shapes', async () => {
  const { backend, host } = await session();
  const taken = new Set();

  const reading = backend.host.credentials.get('connection.prod');
  const get = await nextRequest(host, 'lithe/host/credentials/get', taken);
  assert.deepEqual(get.params, { key: 'connection.prod' });
  host.send({ id: get.id, result: { secret: 's3cret' } });
  assert.equal(await reading, 's3cret');

  const missing = backend.host.credentials.get('connection.prod');
  host.send({ id: (await nextRequest(host, 'lithe/host/credentials/get', taken)).id, result: null });
  assert.equal(await missing, null);

  const malformed = backend.host.credentials.get('connection.prod');
  host.send({ id: (await nextRequest(host, 'lithe/host/credentials/get', taken)).id, result: {} });
  await assert.rejects(malformed, (error) => error.code === 'sdk.host.invalidResult');

  const writing = backend.host.credentials.set('connection.prod:password', 's3cret');
  const set = await nextRequest(host, 'lithe/host/credentials/set', taken);
  assert.deepEqual(set.params, { key: 'connection.prod:password', secret: 's3cret' });
  host.send({ id: set.id, result: null });
  assert.equal(await writing, undefined);

  const deleting = backend.host.credentials.delete('connection.prod');
  const remove = await nextRequest(host, 'lithe/host/credentials/delete', taken);
  assert.deepEqual(remove.params, { key: 'connection.prod' });
  host.send({ id: remove.id, result: null });
  await deleting;

  // Malformed arguments fail locally and never reach the host.
  const before = host.messages.length;
  await assert.rejects(backend.host.credentials.get(''), (error) => error.code === 'sdk.credentials.invalidKey');
  await assert.rejects(backend.host.credentials.get('has space'), (error) => error.code === 'sdk.credentials.invalidKey');
  await assert.rejects(backend.host.credentials.get('k'.repeat(129)), (error) => error.code === 'sdk.credentials.invalidKey');
  await assert.rejects(backend.host.credentials.set('key', 7), (error) => error.code === 'sdk.credentials.invalidSecret');
  await assert.rejects(
    backend.host.credentials.set('key', 'x'.repeat(LIMITS.maxCredentialSecretBytes + 1)),
    (error) => error.code === 'sdk.credentials.invalidSecret',
  );
  assert.equal(host.messages.length, before);
  await host.close();
});

test('local key and context checks agree with the fixture cases Core refuses or forwards', async () => {
  const { backend, host } = await session();
  const key = (testCase) => testCase.params?.key;
  for (const testCase of fixture.hostRequests.filter((testCase) => testCase.method.startsWith('lithe/host/credentials/'))) {
    if (testCase.expected.forward) {
      assert.match(key(testCase), /^[A-Za-z0-9._:-]{1,128}$/, testCase.name);
    } else if (testCase.name.includes('key')) {
      await assert.rejects(backend.host.credentials.get(key(testCase)), (error) => error.code === 'sdk.credentials.invalidKey', testCase.name);
    }
  }
  for (const testCase of fixture.hostRequests.filter((testCase) => /context (without|ID outside)|generation is positive/.test(testCase.name))) {
    await assert.rejects(
      backend.host.workspace.getFolders(testCase.params.context),
      (error) => error.code === 'sdk.host.invalidContext',
      testCase.name,
    );
  }
  // A full context from listContexts is passed on as its reference.
  const asking = backend.host.workspace.getFolders({ ...CONTEXT, name: 'orders', folders: [] });
  const request = await nextRequest(host, 'lithe/host/workspace/getFolders');
  assert.deepEqual(request.params, { context: CONTEXT });
  host.send({ id: request.id, result: { folders: [] } });
  assert.deepEqual(await asking, { folders: [] });
  await host.close();
});

test('host refusals become HostError with the code, number, and details Core sent', async () => {
  const { backend, host } = await session();
  const taken = new Set();
  const answer = async (wire) => {
    const pending = backend.host.request('lithe/host/workspace/listContexts');
    const request = await nextRequest(host, 'lithe/host/workspace/listContexts', taken);
    host.send({ id: request.id, error: wire });
    return pending.then(() => assert.fail('resolved'), (error) => error);
  };
  for (const testCase of fixture.hostRequests.filter(({ expected }) => expected.code)) {
    const { code, rpcCode, details } = testCase.expected;
    const error = await answer({ code: rpcCode, message: 'Refused.', data: { code, ...(details ? { details } : {}) } });
    assert.ok(error instanceof HostError && error instanceof PluginSdkError, testCase.name);
    assert.deepEqual({ code: error.code, rpcCode: error.rpcCode, details: error.details }, { code, rpcCode, details }, testCase.name);
    assert.equal(error.method, 'lithe/host/workspace/listContexts');
  }
  for (const testCase of fixture.hostFailures) {
    const { code, rpcCode } = testCase.expected;
    assert.equal(HOST_ERRORS[code], rpcCode, testCase.name);
    // The number alone names the code.
    assert.equal((await answer({ code: rpcCode, message: 'Failed.' })).code, code, testCase.name);
  }
  const unknown = await answer({ code: -32050, message: 'Later code.', data: { details: ['not', 'an', 'object'] } });
  assert.deepEqual({ code: unknown.code, details: unknown.details }, { code: 'plugin.host.failed', details: undefined });
  await host.close();
});

test('without litheFunctions the host offers no service, and host requests stay within lithe/host/', async () => {
  const backend = createBackend();
  const host = new TestHost(backend);
  await host.initialize({});
  const before = host.messages.length;
  await assert.rejects(
    backend.host.credentials.get('connection.prod'),
    (error) => error instanceof HostError && error.code === 'plugin.host.unavailable' && error.rpcCode === -32601,
  );
  await assert.rejects(backend.host.capabilities(), (error) => error.code === 'plugin.host.unavailable');
  assert.equal(await backend.host.supports('lithe/host/credentials/get'), false);
  await assert.rejects(backend.host.request('lithe/function/invoke', {}), (error) => error.code === 'sdk.host.method');
  assert.equal(host.messages.length, before);
  await host.close();

  const idle = createBackend();
  await assert.rejects(idle.host.credentials.get('key'), (error) => error.code === 'sdk.session.notReady');
});

test('capabilities are asked for once, and supports() reads them', async () => {
  const { backend, host } = await session();
  const answer = fixture.hostRequests.find((testCase) => testCase.expected.answer).expected.answer;
  const first = backend.host.supports('lithe/host/workspace/getFolders');
  const second = backend.host.supports('lithe/host/workspace/readFile');
  const request = await nextRequest(host, 'lithe/host/getCapabilities');
  assert.equal(request.params, undefined);
  host.send({ id: request.id, result: answer });
  assert.deepEqual([await first, await second], [true, false]);
  assert.equal((await backend.host.capabilities()).limits.pendingHostRequests, LIMITS.maxPendingHostRequests);
  assert.equal(host.messages.filter((message) => message.method === 'lithe/host/getCapabilities').length, 1);
  await host.close();
});

test('a call carries its workspace context to the handler', async () => {
  for (const testCase of fixture.invocationContexts.filter(({ expected }) => expected.params)) {
    let seen;
    const backend = createBackend().function(testCase.params.functionID, (_params, context) => {
      seen = context.workspaceContext;
    });
    const { host } = await session(backend);
    const response = await host.request('lithe/function/invoke', testCase.expected.params);
    assert.equal(response.error, undefined, testCase.name);
    assert.deepEqual(seen, testCase.expected.params.context ?? null, testCase.name);
    await host.close();
  }
});

test('cancelling a call cancels its host requests at Core', async () => {
  const started = gate();
  let outcome;
  const backend = createBackend().function('secret.read', async (_params, context) => {
    const reading = context.host.credentials.get('connection.prod');
    started.open();
    outcome = await reading.then(() => 'resolved', (error) => error);
    throw outcome;
  });
  const { host } = await session(backend);
  const call = host.request('lithe/function/invoke', { functionID: 'secret.read', operationID: 'c' }, 41);
  await withTimeout(started.opened, 'the host request');
  const request = await nextRequest(host, 'lithe/host/credentials/get');
  host.notify('$/cancelRequest', { id: 41 });
  assert.equal((await call).error.code, -32800);
  // The host request rejects with the call's abort reason.
  assert.equal(outcome?.name, 'AbortError');
  const cancels = host.messages.filter((message) => message.method === '$/cancelRequest');
  assert.deepEqual(cancels.map((message) => message.params), [{ id: request.id }]);
  // Core's late answer is dropped; the session continues.
  host.send({ id: request.id, error: { code: -32800, message: 'Cancelled.', data: { code: 'plugin.host.cancelled' } } });
  assert.equal(await host.close(), 1);
});

test('an aborted signal rejects with its reason and is never sent again', async () => {
  const { backend, host } = await session();
  const controller = new AbortController();
  const pending = backend.host.editor.applyEdits(CONTEXT, { documentID: 'd1', expectedVersion: 4, edits: [] }, { signal: controller.signal });
  const request = await nextRequest(host, 'lithe/host/editor/applyEdits');
  assert.deepEqual(request.params, { documentID: 'd1', expectedVersion: 4, edits: [], context: CONTEXT });
  controller.abort(new Error('user stopped'));
  await assert.rejects(pending, /user stopped/);
  host.send({ id: request.id, result: { documentID: 'd1', version: 5, applied: true } });
  await assert.rejects(backend.host.request('lithe/host/editor/applyEdits', {}, { signal: controller.signal }), /user stopped/);
  assert.equal(host.messages.filter((message) => message.method === 'lithe/host/editor/applyEdits').length, 1);
  await host.close();
});

test('at most 16 host requests are unanswered; later ones wait for a free slot', async () => {
  const { backend, host } = await session();
  const max = LIMITS.maxPendingHostRequests;
  const calls = Array.from({ length: max + 2 }, () => backend.host.workspace.listContexts());
  const sent = () => host.messages.filter((message) => message.method === 'lithe/host/workspace/listContexts');
  await host.waitFor(() => sent().length === max, 'the first 16 requests');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent().length, max);
  host.send({ id: sent()[0].id, result: { revision: 1, contexts: [] } });
  await host.waitFor(() => sent().length === max + 1, 'the 17th request');
  assert.equal(sent().length, max + 1);
  // Ending the session fails the queued request too.
  await host.close();
  const outcomes = await Promise.allSettled(calls);
  assert.equal(outcomes[0].status, 'fulfilled');
  assert.ok(outcomes.slice(1).every((outcome) => outcome.status === 'rejected' && outcome.reason.code === 'sdk.session.closed'));
});

test('subscription events reach their handler in order, also when sent before the answer', async () => {
  const { backend, host } = await session();
  const events = [];
  const received = gate();
  const watching = backend.host.workspace.watch(CONTEXT, { resource: { rootID: 'r1', path: '' }, recursive: true }, (event, info) => {
    events.push({ event, ...info });
    if (events.length === 3) received.open();
  });
  const request = await nextRequest(host, 'lithe/host/workspace/watch');
  assert.deepEqual(request.params, { resource: { rootID: 'r1', path: '' }, recursive: true, context: CONTEXT });
  const notify = (sequence) => host.notify('lithe/host/subscriptions/notify', {
    subscriptionID: 's1', sequence, event: { rootID: 'r1', changedPaths: [`f${sequence}`], requiresRescan: false },
  });
  notify(1);
  host.send({ id: request.id, result: { subscriptionID: 's1' } });
  const subscription = await watching;
  notify(2);
  notify(4);
  await withTimeout(received.opened, 'three events');
  assert.deepEqual(events.map(({ sequence, gap, subscriptionID }) => ({ sequence, gap, subscriptionID })), [
    { sequence: 1, gap: false, subscriptionID: 's1' },
    { sequence: 2, gap: false, subscriptionID: 's1' },
    { sequence: 4, gap: true, subscriptionID: 's1' },
  ]);
  assert.equal(subscription.subscriptionID, 's1');
  assert.equal(subscription.disposed, false);

  const disposing = subscription.dispose();
  assert.equal(subscription.disposed, true);
  const dispose = await nextRequest(host, 'lithe/host/subscriptions/dispose');
  assert.deepEqual(dispose.params, { subscriptionID: 's1' });
  notify(5);
  host.send({ id: dispose.id, result: null });
  await disposing;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(events.length, 3);
  await host.close();
});

test('commands, context changes, and later host notifications reach their handlers', async () => {
  const COMMAND_DID_RUN = 'lithe/host/commands/didRun';
  const CONTEXTS_DID_CHANGE = 'lithe/host/workspace/didChangeContexts';
  const delivered = fixture.hostNotifications.filter(({ expected, method }) => expected.delivered && method !== 'lithe/host/subscriptions/notify');
  // Commands and context changes have their own API. The other
  // notifications (a run's exit, a language server's state and notifications)
  // have no wrapper; the generic handler delivers each with its params as sent.
  const expected = [
    ...delivered.map(({ method, params }) => {
      if (method === COMMAND_DID_RUN) return ['command', { commandID: params.commandID, workspaceContext: params.context ?? null }];
      if (method === CONTEXTS_DID_CHANGE) return ['contexts', params];
      return [method, params];
    }),
    ['later', { value: 1 }],
  ];
  const backend = createBackend();
  const seen = [];
  const all = gate();
  const record = (entry) => {
    seen.push(entry);
    if (seen.length === expected.length) all.open();
  };
  backend
    .onCommand('db.refresh', (event) => record(['command', event]))
    .onContextsChanged((event) => record(['contexts', event]))
    .onHostNotification('lithe/host/later/didSomething', (params) => record(['later', params]));
  // A method gets one generic handler, however many fixture cases use it.
  for (const method of new Set(delivered.map((entry) => entry.method))) {
    if (method === COMMAND_DID_RUN || method === CONTEXTS_DID_CHANGE) continue;
    backend.onHostNotification(method, (params) => record([method, params]));
  }
  assert.throws(() => backend.onHostNotification('lithe/host/commands/didRun', () => {}), (error) => error.code === 'sdk.method.reserved');
  assert.throws(() => backend.onHostNotification('workspace/didChange', () => {}), (error) => error.code === 'sdk.method.invalid');
  assert.throws(() => backend.onCommand('DB_Refresh', () => {}), (error) => error.code === 'sdk.command.invalidID');
  const { host } = await session(backend);
  for (const { method, params } of delivered) host.notify(method, params);
  // A notification this SDK does not know is ignored, as the contract asks.
  host.notify('lithe/host/unknown/ignored', { anything: true });
  host.notify('lithe/host/later/didSomething', { value: 1 });
  await withTimeout(all.opened, 'the notifications');
  assert.deepEqual(seen, expected);
  await host.close();
});

test('native terminal wrappers keep argv and context, and subscribe to one owned exit', { timeout: 5000 }, async () => {
  const { backend, host } = await session();
  try {
    const params = { executable: 'example-client', arguments: ['literal;$(value)', 'space value'], show: true };
    const creating = backend.host.terminal.create(CONTEXT, params);
    const create = await nextRequest(host, 'lithe/host/terminal/create');
    assert.deepEqual(create.params, { ...params, context: CONTEXT });
    host.send({ id: create.id, result: { terminalID: 'owned', generation: 1 } });
    assert.deepEqual(await creating, { terminalID: 'owned', generation: 1 });
    const seen = gate();
    let event;
    const listening = backend.host.terminal.onExit(CONTEXT, 'owned', value => { event = value; seen.open(); });
    const listen = await nextRequest(host, 'lithe/host/terminal/onExit');
    host.send({ id: listen.id, result: { subscriptionID: 'terminal-exit' } });
    const subscription = await listening;
    host.notify('lithe/host/subscriptions/notify', { subscriptionID: 'terminal-exit', sequence: 1,
      event: { type: 'exit', terminalID: 'owned', exitCode: 0, reason: 'exited' } });
    await withTimeout(seen.opened, 'terminal exit event');
    assert.equal(event.exitCode, 0);
    const closing = backend.host.terminal.close(CONTEXT, { terminalID: 'owned' });
    const close = await nextRequest(host, 'lithe/host/terminal/close');
    host.send({ id: close.id, result: { retainedContextBytes: 0 } });
    assert.deepEqual(await closing, { retainedContextBytes: 0 });
    const reopening = backend.host.terminal.reopen(CONTEXT, { terminalID: 'owned' });
    const reopen = await nextRequest(host, 'lithe/host/terminal/reopen');
    host.send({ id: reopen.id, result: { terminalID: 'owned', generation: 2, restoredContextBytes: 0 } });
    assert.equal((await reopening).generation, 2);
    const disposing = subscription.dispose();
    const dispose = await nextRequest(host, 'lithe/host/subscriptions/dispose');
    host.send({ id: dispose.id, result: null });
    await disposing;
  } finally { await host.close(); }
});

const RELAY_PROVIDERS = 'lithe/host/functions/providers';
const RELAY_INVOKE = 'lithe/host/functions/invoke';
/** The provider and function the fixture's relay cases name. */
const PROVIDER = Object.freeze({ pluginID: 'dev.lithe.fixture.go', functionID: 'languageImport.settings' });

/** The fixture's host requests for one method. */
const relayCases = (method) => fixture.hostRequests.filter((testCase) => testCase.method === method);

/** A fixture invoke case as the SDK takes it: the context, and the call without it. */
function splitInvoke({ params }) {
  const { context, ...call } = params;
  return { context, call };
}

test('context.caller is the relayed caller, read only from the top-level field', async () => {
  // The fixture's cases: the wire message Core sends, and whether it holds a caller.
  for (const testCase of fixture.invocationCallers) {
    const seen = [];
    const backend = createBackend().function(testCase.params.functionID, (_params, context) => {
      seen.push(context.caller);
    });
    const { host } = await session(backend);
    const response = await host.request('lithe/function/invoke', testCase.expected.params);
    assert.equal(response.error, undefined, testCase.name);
    assert.deepEqual(seen, [testCase.expected.params.caller ?? null], testCase.name);
    if (seen[0] !== null) assert.ok(Object.isFrozen(seen[0]), testCase.name);
    await host.close();
  }

  // Anything but exactly { pluginID: non-empty string } at the top level is no caller.
  const wire = { functionID: 'languageImport.settings', operationID: '7', params: {} };
  const seen = [];
  const backend = createBackend().function('languageImport.settings', (params, context) => {
    seen.push({ caller: context.caller, params });
  });
  const { host } = await session(backend);
  const notCallers = [
    { caller: { pluginID: '' } },
    { caller: { pluginID: 7 } },
    { caller: {} },
    { caller: { pluginID: 'dev.lithe.fixture.python', extra: true } },
    { caller: 'dev.lithe.fixture.python' },
    { caller: null },
    { caller: ['dev.lithe.fixture.python'] },
    // A caller inside params is the calling plugin's own data.
    { params: { caller: { pluginID: 'dev.lithe.fixture.other' } } },
  ];
  for (const extra of notCallers) {
    const response = await host.request('lithe/function/invoke', { ...wire, ...extra });
    assert.equal(response.error, undefined, JSON.stringify(extra));
  }
  assert.deepEqual(seen.map((entry) => entry.caller), notCallers.map(() => null));
  assert.deepEqual(seen.at(-1).params, { caller: { pluginID: 'dev.lithe.fixture.other' } });
  await host.close();
});

test('functions.providers sends the function ID and returns the host answer unwrapped', async () => {
  const { backend, host } = await session();
  const taken = new Set();
  const testCase = relayCases(RELAY_PROVIDERS).find(({ expected }) => expected.relay);
  const listing = backend.host.functions.providers(testCase.params.functionID);
  const request = await nextRequest(host, RELAY_PROVIDERS, taken);
  assert.deepEqual(request.params, testCase.params);
  const answer = { providers: [{ pluginID: 'dev.lithe.fixture.go', version: 1 }, { pluginID: 'dev.lithe.fixture.rust', version: 1 }] };
  host.send({ id: request.id, result: answer });
  assert.deepEqual(await listing, answer);

  const none = backend.host.functions.providers(testCase.params.functionID);
  host.send({ id: (await nextRequest(host, RELAY_PROVIDERS, taken)).id, result: { providers: [] } });
  assert.deepEqual(await none, { providers: [] });
  await host.close();
});

test('functions.invoke sends the fixture params and returns the host answer unwrapped', async () => {
  const { backend, host } = await session();
  const taken = new Set();
  const cases = relayCases(RELAY_INVOKE).filter(({ expected }) => expected.relay);
  assert.ok(cases.some(({ params }) => params.parentOperationID !== undefined));
  assert.ok(cases.some(({ params }) => params.parentOperationID === undefined));
  for (const testCase of cases) {
    const { context, call } = splitInvoke(testCase);
    for (const value of [{ answer: 42 }, null]) {
      const calling = backend.host.functions.invoke(context, call);
      const request = await nextRequest(host, RELAY_INVOKE, taken);
      // backend.host serves no call, so only an explicit parent is sent.
      assert.deepEqual(request.params, testCase.params, testCase.name);
      host.send({ id: request.id, result: { value } });
      assert.deepEqual(await calling, { value }, testCase.name);
    }
  }
  // A full context from listContexts is passed on as its reference.
  const calling = backend.host.functions.invoke({ ...CONTEXT, name: 'orders', folders: [] }, PROVIDER);
  const request = await nextRequest(host, RELAY_INVOKE, taken);
  assert.deepEqual(request.params, { ...PROVIDER, context: CONTEXT });
  host.send({ id: request.id, result: { value: 'ok' } });
  assert.deepEqual(await calling, { value: 'ok' });
  await host.close();
});

test('inside a function, relayed calls default to the served call as their parent', async () => {
  const backend = createBackend().function('project.describe', async ({ via, call }, context) => {
    const caller = via === 'backend' ? backend.host : context.host;
    await caller.functions.invoke(CONTEXT, call);
    return null;
  });
  const { host } = await session(backend);
  const taken = new Set();
  const cases = [
    ['the served call', { operationID: 'op-1' }, 'context', PROVIDER, 'op-1'],
    ['an explicit parent wins', { operationID: 'op-1' }, 'context', { ...PROVIDER, parentOperationID: 'explicit' }, 'explicit'],
    ['backend.host has no served call', { operationID: 'op-1' }, 'backend', PROVIDER, undefined],
    ['no wire operationID', {}, 'context', PROVIDER, undefined],
    ['an operationID that is not a string', { operationID: 7 }, 'context', PROVIDER, undefined],
    ['an empty operationID', { operationID: '' }, 'context', PROVIDER, undefined],
    ['an operationID of 129 bytes', { operationID: 'o'.repeat(129) }, 'context', PROVIDER, undefined],
    ['an operationID of 128 bytes', { operationID: 'o'.repeat(128) }, 'context', PROVIDER, 'o'.repeat(128)],
    ['an operationID of 128 bytes in 64 characters', { operationID: 'é'.repeat(64) }, 'context', PROVIDER, 'é'.repeat(64)],
    ['an operationID of 130 bytes in 65 characters', { operationID: 'é'.repeat(65) }, 'context', PROVIDER, undefined],
  ];
  for (const [name, wire, via, call, parent] of cases) {
    const done = host.request('lithe/function/invoke', { functionID: 'project.describe', params: { via, call }, ...wire });
    const request = await nextRequest(host, RELAY_INVOKE, taken);
    if (parent === undefined) {
      assert.ok(!('parentOperationID' in request.params), name);
    } else {
      assert.equal(request.params.parentOperationID, parent, name);
    }
    host.send({ id: request.id, result: { value: null } });
    assert.equal((await done).error, undefined, name);
  }
  await host.close();
});

test('relayed calls from build and request handlers carry no default parent', async () => {
  const backend = createBackend();
  backend.build.execute(async (_params, context) => {
    await context.host.functions.invoke(CONTEXT, PROVIDER);
    return { exitCode: 0, output: 'built\n' };
  });
  backend.onRequest('example/relay', async (_params, context) => {
    await context.host.functions.invoke(CONTEXT, PROVIDER);
    return null;
  });
  const host = new TestHost(backend);
  try {
    await host.initialize({ litheFunctions: { versions: [0] }, litheBuild: { versions: [0] } });
    const taken = new Set();
    for (const [method, params] of [['lithe/build/execute', { operationID: 'wire-execute' }], ['example/relay', { operationID: 'wire-relay' }]]) {
      const done = host.request(method, params);
      const request = await nextRequest(host, RELAY_INVOKE, taken);
      assert.ok(!('parentOperationID' in request.params), method);
      host.send({ id: request.id, result: { value: null } });
      assert.equal((await done).error, undefined, method);
    }
  } finally { await host.close(); }
});

test('supports() tells a plugin whether the host lists the relay methods', async () => {
  const listed = await session();
  const asking = listed.backend.host.supports(RELAY_INVOKE);
  const request = await nextRequest(listed.host, 'lithe/host/getCapabilities');
  listed.host.send({ id: request.id, result: { services: { functions: { methods: [RELAY_PROVIDERS, RELAY_INVOKE] } } } });
  assert.equal(await asking, true);
  assert.equal(await listed.backend.host.supports(RELAY_PROVIDERS), true);
  await listed.host.close();

  const unlisted = await session();
  const unlistedAsking = unlisted.backend.host.supports(RELAY_INVOKE);
  const unlistedRequest = await nextRequest(unlisted.host, 'lithe/host/getCapabilities');
  unlisted.host.send({
    id: unlistedRequest.id,
    result: { services: { workspace: { methods: ['lithe/host/workspace/listContexts'] } } },
  });
  assert.equal(await unlistedAsking, false);
  await unlisted.host.close();
});

test('relayed calls are checked locally, and a mistake sends no frame', async () => {
  const { backend, host } = await session();
  const before = host.messages.length;
  const refusedWith = (code) => (error) => error instanceof PluginSdkError && error.code === code;

  // The fixture's calls that Core would answer -32602.
  const refusals = relayCases(RELAY_INVOKE).filter(({ expected }) => expected.code === 'plugin.host.invalidParams');
  assert.ok(refusals.length >= 5);
  for (const testCase of refusals) {
    const { context, call } = splitInvoke(testCase);
    const code = context === undefined ? 'sdk.host.invalidContext' : 'sdk.functions.invalidCall';
    await assert.rejects(backend.host.functions.invoke(context, call), refusedWith(code), testCase.name);
  }

  const good = { ...PROVIDER };
  const badCalls = [
    ['no call', undefined],
    ['a null call', null],
    ['an array for a call', []],
    ['an empty plugin ID', { ...good, pluginID: '' }],
    ['a plugin ID that is not a string', { ...good, pluginID: 7 }],
    ['no plugin ID', { functionID: good.functionID }],
    ['an empty function ID', { ...good, functionID: '' }],
    ['a function ID over 128 bytes', { ...good, functionID: 'a'.repeat(129) }],
    ['null params', { ...good, params: null }],
    ['string params', { ...good, params: 'x' }],
    ['array params', { ...good, params: [] }],
    ['a fractional timeout', { ...good, timeoutMilliseconds: 1.5 }],
    ['a negative timeout', { ...good, timeoutMilliseconds: -1 }],
    ['a timeout of NaN', { ...good, timeoutMilliseconds: Number.NaN }],
    ['a timeout of Infinity', { ...good, timeoutMilliseconds: Number.POSITIVE_INFINITY }],
    ['a timeout string', { ...good, timeoutMilliseconds: '5000' }],
    ['a timeout beyond a safe integer', { ...good, timeoutMilliseconds: Number.MAX_SAFE_INTEGER + 1 }],
    ['a null parent', { ...good, parentOperationID: null }],
    ['an empty parent', { ...good, parentOperationID: '' }],
    ['a parent of 129 bytes', { ...good, parentOperationID: 'p'.repeat(129) }],
    ['a parent of 130 bytes in 65 characters', { ...good, parentOperationID: 'é'.repeat(65) }],
  ];
  for (const [name, call] of badCalls) {
    await assert.rejects(backend.host.functions.invoke(CONTEXT, call), refusedWith('sdk.functions.invalidCall'), name);
  }
  for (const context of [undefined, null, {}, { contextID: 'w1' }, { contextID: 'w1', generation: 0 }, { contextID: '../w', generation: 1 }]) {
    await assert.rejects(backend.host.functions.invoke(context, good), refusedWith('sdk.host.invalidContext'), `context ${JSON.stringify(context)}`);
  }
  for (const functionID of [undefined, 7, '', 'Not-Valid', 'a..b', 'a'.repeat(129)]) {
    await assert.rejects(backend.host.functions.providers(functionID), refusedWith('sdk.functions.invalidCall'), `function ID ${String(functionID)}`);
  }
  assert.equal(host.messages.length, before);

  // Limits Core enforces are not enforced twice: a long timeout is passed on as given.
  const taken = new Set();
  const plain = backend.host.functions.invoke(CONTEXT, good);
  const plainRequest = await nextRequest(host, RELAY_INVOKE, taken);
  assert.deepEqual(Object.keys(plainRequest.params).sort(), ['context', 'functionID', 'pluginID']);
  host.send({ id: plainRequest.id, result: { value: null } });
  await plain;
  const long = backend.host.functions.invoke(CONTEXT, { ...good, timeoutMilliseconds: LIMITS.relayMaxTimeoutMilliseconds + 1 });
  const longRequest = await nextRequest(host, RELAY_INVOKE, taken);
  assert.equal(longRequest.params.timeoutMilliseconds, LIMITS.relayMaxTimeoutMilliseconds + 1);
  host.send({ id: longRequest.id, result: { value: null } });
  await long;
  assert.deepEqual(
    [LIMITS.relayDefaultTimeoutMilliseconds, LIMITS.relayMaxTimeoutMilliseconds, LIMITS.maxParentOperationIdBytes],
    [60000, 600000, 128],
  );
  await host.close();
});

test('the wire params of a relayed call hold only the fields the contract defines', async () => {
  const { backend, host } = await session();
  // The fixture case where a caller tries to choose who the callee thinks called.
  const injection = relayCases(RELAY_INVOKE).find(({ params }) => 'caller' in params);
  const { context, call } = splitInvoke(injection);
  assert.deepEqual(call.caller, { pluginID: 'dev.lithe.fixture.other' });
  const calling = backend.host.functions.invoke(context, { ...call, unknown: true });
  const request = await nextRequest(host, RELAY_INVOKE);
  assert.deepEqual(request.params, { pluginID: call.pluginID, functionID: call.functionID, context });
  host.send({ id: request.id, result: { value: null } });
  await calling;
  await host.close();
});

test('relay failures become HostError by code, by number alone, and with the provider details', async () => {
  const { backend, host } = await session();
  const taken = new Set();
  const failWith = async (wire) => {
    const calling = backend.host.functions.invoke(CONTEXT, PROVIDER);
    const request = await nextRequest(host, RELAY_INVOKE, taken);
    host.send({ id: request.id, error: wire });
    return calling.then(() => assert.fail('resolved'), (error) => error);
  };
  assert.ok(fixture.relayErrors.length > 0);
  for (const { name, code, rpcCode } of fixture.relayErrors) {
    assert.equal(HOST_ERRORS[code], rpcCode, name);
    const byCode = await failWith({ code: rpcCode, message: 'Refused.', data: { code } });
    const byNumber = await failWith({ code: rpcCode, message: 'Refused.' });
    for (const error of [byCode, byNumber]) {
      assert.ok(error instanceof HostError && error instanceof PluginSdkError, name);
      assert.deepEqual({ code: error.code, rpcCode: error.rpcCode, method: error.method }, { code, rpcCode, method: RELAY_INVOKE }, name);
    }
  }

  // The provider's own failure keeps its message and its code in the details.
  const failed = await failWith({
    code: -32070,
    message: 'The snapshot is locked.',
    data: { code: 'plugin.function.failed', details: { pluginErrorCode: 'maven.snapshotLocked' } },
  });
  assert.equal(failed.code, 'plugin.function.failed');
  assert.equal(failed.message, 'The snapshot is locked.');
  assert.deepEqual(failed.details, { pluginErrorCode: 'maven.snapshotLocked' });
  await host.close();
});

test('an aborted signal cancels a relayed call at Core, and an aborted one is never sent', async () => {
  const { backend, host } = await session();
  const controller = new AbortController();
  const pending = backend.host.functions.invoke(CONTEXT, PROVIDER, { signal: controller.signal });
  const request = await nextRequest(host, RELAY_INVOKE);
  controller.abort(new Error('user stopped'));
  await assert.rejects(pending, /user stopped/);
  const cancel = await host.waitFor(
    (message) => message.method === '$/cancelRequest' && message.params?.id === request.id,
    '$/cancelRequest',
  );
  assert.deepEqual(cancel.params, { id: request.id });
  // Core's late answer is dropped.
  host.send({ id: request.id, error: { code: -32800, message: 'Cancelled.', data: { code: 'plugin.host.cancelled' } } });
  const before = host.messages.length;
  await assert.rejects(backend.host.functions.invoke(CONTEXT, PROVIDER, { signal: controller.signal }), /user stopped/);
  assert.equal(host.messages.length, before);
  await host.close();
});

test('cancelling the served call cancels its relayed call', async () => {
  const started = gate();
  let outcome;
  const backend = createBackend().function('project.describe', async (_params, context) => {
    const calling = context.host.functions.invoke(CONTEXT, PROVIDER);
    started.open();
    outcome = await calling.then(() => 'resolved', (error) => error);
    throw outcome;
  });
  const { host } = await session(backend);
  const call = host.request('lithe/function/invoke', { functionID: 'project.describe', operationID: 'c' }, 41);
  await withTimeout(started.opened, 'the relayed call');
  const request = await nextRequest(host, RELAY_INVOKE);
  assert.equal(request.params.parentOperationID, 'c');
  host.notify('$/cancelRequest', { id: 41 });
  assert.equal((await call).error.code, -32800);
  assert.equal(outcome?.name, 'AbortError');
  const cancels = host.messages.filter((message) => message.method === '$/cancelRequest');
  assert.deepEqual(cancels.map((message) => message.params), [{ id: request.id }]);
  // Core's late answer is dropped; the session continues.
  host.send({ id: request.id, error: { code: -32800, message: 'Cancelled.', data: { code: 'plugin.host.cancelled' } } });
  await host.close();
});

test('a relayed call holds one of the 16 host request slots until Core answers', async () => {
  const { backend, host } = await session();
  const max = LIMITS.maxPendingHostRequests;
  const calls = Array.from({ length: max + 2 }, () => backend.host.functions.invoke(CONTEXT, PROVIDER));
  // Settled before the session ends, so no rejection goes unobserved.
  const settled = Promise.allSettled(calls);
  const sent = () => host.messages.filter((message) => message.method === RELAY_INVOKE);
  await host.waitFor(() => sent().length === max, 'the first 16 relayed calls');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent().length, max);
  host.send({ id: sent()[0].id, result: { value: null } });
  await host.waitFor(() => sent().length === max + 1, 'the 17th relayed call');
  assert.equal(sent().length, max + 1);
  // Ending the session fails the queued call too.
  await host.close();
  const outcomes = await settled;
  assert.equal(outcomes[0].status, 'fulfilled');
  assert.ok(outcomes.slice(1).every((outcome) => outcome.status === 'rejected' && outcome.reason.code === 'sdk.session.closed'));
});
