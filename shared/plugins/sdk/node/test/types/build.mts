// Check the published package's exports, including contextual return types.
import { createBackend, type BuildContext, type BuildRequestParams } from '@lithe/plugin-sdk';

const backend = createBackend();
const version: number | null = backend.buildVersion;
void version;
backend.build.execute((params, context) => {
  const task: string = params.task;
  const operation: string = context.operationID;
  context.progress({ message: task, output: operation, percentage: 50 });
  return { exitCode: 0, output: params.userArguments.join(' ') };
});
backend.build.test(async (params, context) => {
  const suite: string | undefined = params.suite;
  context.progress({ output: suite });
  return { exitCode: 2, output: 'test failure' };
});
backend.build.discoverTests(async (params: BuildRequestParams) => ({
  tests: [{ suite: 'Suite', case: 'exact # [case]', project: { name: 'app', path: params.root } }],
}));
backend.build.prepareRun(async (params, context: BuildContext) => {
  context.throwIfCancelled();
  return { executable: { kind: 'toolchain', value: 'project-jdk' },
    arguments: ['Main', ...params.userArguments], workingDirectory: params.project.path, environment: {} };
});
backend.build.model((_params, context) => {
  // @ts-expect-error Model requests have no bounded execution progress.
  context.progress({ message: 'invalid' });
  return { projects: [] };
});
// @ts-expect-error The exit code is a number, not a shell/string result.
backend.build.execute(() => ({ exitCode: '0', output: '' }));
// @ts-expect-error Every discovered selector needs an exact case.
backend.build.discoverTests(() => ({ tests: [{ suite: 'incomplete' }] }));
// @ts-expect-error Arbitrary executable kinds are not portable launch plans.
backend.build.prepareRun(() => ({ executable: { kind: 'absolute', value: '/fixture/tool' },
  arguments: [], workingDirectory: '.', environment: {} }));

backend.build.prepareDebug((params, context) => {
  context.progress({ output: params.run });
  return { program: 'app/bin/main', arguments: params.userArguments,
    workingDirectory: params.project.path, environment: {} };
});
// @ts-expect-error A native debug target requires a program path.
backend.build.prepareDebug(() => ({ arguments: [], workingDirectory: '.', environment: {} }));

backend.build.model((params) => {
  for (const slot of params.modelInputs?.inputs ?? []) {
    const producer: string = slot.pluginID;
    void producer;
    for (const instance of slot.instances) {
      if (instance.state === 'ready') {
        const projects: string[] = instance.model.projects.map((project) => project.id);
        void projects;
        // @ts-expect-error Public inputs never carry executable tasks.
        void instance.model.tasks;
      } else {
        // @ts-expect-error Old facts are absent while inputs are stale/loading/failed.
        void instance.model;
      }
    }
  }
  return { projects: [] };
});
