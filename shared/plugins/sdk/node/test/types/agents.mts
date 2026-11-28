// Check the agent-session declarations the way an agent plugin uses them.
import {
  createBackend,
  HOST_METHODS,
  type AgentSession,
  type AgentSessionHandle,
  type AgentSessionStart,
  type AgentUsageResult,
} from '@lithe/plugin-sdk';

const backend = createBackend();
const usageMethod: string = HOST_METHODS.agentsUsage;
void usageMethod;

backend.onAgentSessions((start: AgentSessionStart, session: AgentSessionHandle): AgentSession => {
  const resume: string | null = start.resume;
  void resume;
  return {
    async output(lines: string[]) {
      for (const line of lines) {
        await session.report([{ kind: 'message', turn: 1, role: 'assistant', messageID: 'm1', text: line, delta: true }]);
      }
    },
    async prompt(turn: number, text: string) {
      await session.write([JSON.stringify({ type: 'user', turn, text })]);
    },
    async decidePermission(turn: number, permission: string, option: string) {
      await session.write([JSON.stringify({ turn, permission, option })]);
    },
    exited(exitCode: number | null) {
      void exitCode;
    },
  };
});

backend.function('usage.today', async (_params, context) => {
  const usage: AgentUsageResult = await context.host.agents.usage({ groupBy: 'day', utcOffsetSeconds: 28800 });
  return { summaries: usage.summaries.length };
});

// @ts-expect-error groupBy is day, agent or model.
void backend.host.agents.usage({ groupBy: 'session' });
// @ts-expect-error a handler answers prompts.
backend.onAgentSessions(() => ({ output() {}, decidePermission() {} }));
