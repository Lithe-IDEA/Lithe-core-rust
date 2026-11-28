// Native agent sessions and the agents host service (plan API-45), driven by
// the test host and contract/v1/fixtures/agents/native-session.json.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createBackend } from '../src/index.mjs';
import { gate, TestHost, withTimeout } from './harness.mjs';

const fixture = JSON.parse(
  readFileSync(new URL('../../../contract/v1/fixtures/agents/native-session.json', import.meta.url), 'utf8'),
);

/**
 * A backend whose handlers record every call as `[session, call, args...]`,
 * as the fixture lists them. It fails to start sessions in /work/broken and
 * fails prompts that say fail. agent-1's first output waits until agent-2
 * exited, so sessions sharing one queue never get past it.
 */
function recordingBackend() {
  const calls = [];
  const waiters = [];
  const otherExited = gate();
  const backend = createBackend({ name: 'agents-test' });
  backend.onAgentSessions(async (start, session) => {
    assert.equal(session.id, start.session);
    if (start.cwd === '/work/broken') throw new Error('start_failed');
    const record = (...call) => {
      calls.push([start.session, ...call]);
      for (const waiter of waiters.splice(0)) waiter();
    };
    record('start', { ...start });
    return {
      async output(lines) {
        if (start.session === 'agent-1' && lines.includes('one')) {
          await withTimeout(otherExited.opened, 'agent-2 to exit while agent-1 waits');
        }
        record('output', lines);
      },
      async prompt(turn, text) {
        record('prompt', turn, text);
        if (text === 'fail') throw new Error('prompt_failed');
      },
      cancel(turn) {
        record('cancel', turn);
      },
      decidePermission(turn, permission, option) {
        record('decidePermission', turn, permission, option);
      },
      setConfigOption(option, value) {
        record('setConfigOption', option, value);
      },
      exited(exitCode) {
        record('exited', exitCode);
        if (start.session === 'agent-2') otherExited.open();
      },
    };
  });

  /**
   * The calls of every session once `wanted` calls arrived. Every session's
   * last call is its exit, so nothing follows the count.
   */
  async function bySession(wanted) {
    await withTimeout(new Promise((resolve) => {
      const check = () => (calls.length >= wanted ? resolve() : waiters.push(check));
      check();
    }), `${wanted} session calls`);
    const sessions = {};
    for (const [session, ...call] of calls) (sessions[session] ??= []).push(call);
    return sessions;
  }

  return { backend, bySession };
}

function sendAll(host, notifications) {
  for (const { method, params } of notifications) host.notify(method, params);
}

function count(expected) {
  return Object.values(expected).reduce((sum, calls) => sum + calls.length, 0);
}

function failureCodes(host) {
  return host.messages
    .filter((message) => message.method === 'lithe/log' && message.params.kind === 'operationFailed')
    .map((message) => message.params.code);
}

test('each agent session gets its notifications in order from start to exit', async () => {
  const { backend, bySession } = recordingBackend();
  const host = new TestHost(backend);
  await host.initialize();
  sendAll(host, fixture.sessions.notifications);
  const expected = fixture.sessions.calls;
  assert.deepEqual(await bySession(count(expected)), expected);
  assert.equal(await host.close(), 1);
  // agent-9 never started, so its output reaches no handler.
  assert.deepEqual(failureCodes(host), ['agent_session_unknown']);
});

test('a failed agent start or prompt ends the turn as failed', async () => {
  const { backend, bySession } = recordingBackend();
  const host = new TestHost(backend);
  await host.initialize();
  const { failures } = fixture;
  sendAll(host, failures.notifications);
  // Each session waits for Core's answer, so answer the reports as they come.
  const answered = new Set();
  const reports = [];
  for (let index = 0; index < failures.reports.length; index += 1) {
    const report = await host.waitFor(
      (message) => message.method === 'lithe/host/agents/report' && !answered.has(message.id),
      'a turnEnded report',
    );
    answered.add(report.id);
    reports.push(report.params);
    host.send({ id: report.id, result: {} });
  }
  const sorted = (list) => list.map((report) => JSON.stringify(report)).sort();
  assert.deepEqual(sorted(reports), sorted(failures.reports));
  assert.deepEqual(await bySession(count(failures.calls)), failures.calls);
  assert.equal(await host.close(), 1);
});

test('agent host methods send the fixture requests', async () => {
  const backend = createBackend({ name: 'agents-test' });
  const host = new TestHost(backend);
  await host.initialize();
  for (const entry of fixture.methods) {
    const before = host.messages.length;
    const call = entry.call === 'write'
      ? backend.host.agents.write(entry.session, entry.lines)
      : entry.call === 'report'
        ? backend.host.agents.report(entry.session, entry.events)
        : backend.host.agents.usage(entry.params);
    const request = await host.waitFor(
      (message) => host.messages.indexOf(message) >= before && message.method === entry.request.method,
      entry.request.method,
    );
    assert.deepEqual(request.params, entry.request.params);
    const answer = entry.call === 'usage' ? { sinceMs: 1, untilMs: 2, summaries: [], rateLimits: [] } : {};
    host.send({ id: request.id, result: answer });
    assert.deepEqual(await withTimeout(call, entry.call), entry.call === 'usage' ? answer : undefined);
  }
  assert.equal(await host.close(), 1);
});

test('agent notifications are routed only through agent sessions', async () => {
  const backend = createBackend({ name: 'agents-test' });
  for (const method of ['lithe/host/agents/didStart', 'lithe/host/agents/didOutput', 'lithe/host/agents/didExit']) {
    assert.throws(() => backend.onHostNotification(method, () => {}), { code: 'sdk.method.reserved' });
  }
  // Without a registration a started session is only logged.
  const host = new TestHost(backend);
  await host.initialize();
  host.notify('lithe/host/agents/didStart', { session: 'agent-1', agentID: 'example', cwd: '/work/orders' });
  host.notify('lithe/host/agents/didOutput', { session: 'agent-1', lines: ['one'] });
  // The backend reads both before it reads the end of the input.
  assert.equal(await host.close(), 1);
  assert.deepEqual(failureCodes(host), ['agent_unhandled']);
});
