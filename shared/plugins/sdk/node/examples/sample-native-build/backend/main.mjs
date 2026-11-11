// A fictional build engine whose facts are supplied by lithe-sample.json.
// Native IDE operations use typed handlers; this plugin needs no web page.
import { readFile, realpath } from 'node:fs/promises';
import { join, isAbsolute, resolve, sep, relative, posix } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createBackend, PluginError } from '../../../src/index.mjs';

const backend = createBackend({ name: 'sample-native-build' });
let workspace;
backend.onInitialize(async ({ params }) => { workspace = await realpath(fileURLToPath(params.rootUri)); });

backend.build.model(async ({ root, modelInputs }) => {
  const all = await projects(root);
  return { projects: all.map(({ name, modelPath: path }) => ({ name, path, ...(modelInputs ? { properties: { inputRevision: String(modelInputs.revision), inputProjects: String(modelInputs.inputs.flatMap((slot) => slot.instances).filter((input) => input.state === "ready").flatMap((input) => input.model.projects).length) } } : {}) })),
    targets: all.filter(({ run }) => run).map(({ modelPath: path }) => ({ name: 'Main', path, project: path, tags: ['application'] })) };
});
backend.build.dependencies(() => ({ dependencies: [] }));

backend.build.execute(async (params, context) => {
  if (params.task !== 'build') throw new PluginError('sample.task', 'Unknown build task.');
  return build(await select(params), params.userArguments, context);
});

backend.build.discoverTests(async (params) => ({ tests: (await select(params)).flatMap((project) =>
  (project.tests ?? []).map(({ suite, case: testCase }) => ({ suite, case: testCase,
    project: { name: project.name, path: project.path } }))),
}));

backend.build.test(async (params, context) => {
  const selected = (await select(params)).flatMap((project) => (project.tests ?? [])
    .filter((test) => (params.suite === undefined || params.suite === test.suite)
      && (params.case === undefined || params.case === test.case))
    .map((test) => ({ project, test })));
  if (selected.length === 0 && (params.suite !== undefined || params.case !== undefined)) {
    throw new PluginError('sample.test', 'The selected test no longer exists.');
  }
  let output = '';
  for (const { project, test } of selected) {
    const result = await run(test.command, project, params.userArguments, context);
    output += result.stdout + result.stderr;
    if (result.exitCode !== 0) return { exitCode: result.exitCode, output };
  }
  return { exitCode: 0, output };
});

backend.build.prepareRun((params, context) => prepare(params, context, false));
backend.build.prepareDebug((params, context) => prepare(params, context, true));
async function prepare(params, context, debug) {
  const selected = await select(params);
  const project = selected[0];
  if (selected.length !== 1 || params.run !== 'application' || !project.run
      || params.target?.name !== 'Main' || params.target?.path !== project.path) {
    throw new PluginError('sample.target', 'The selected application no longer exists.');
  }
  const result = await build(selected, [], context);
  if (result.exitCode !== 0) throw new PluginError('sample.buildFailed', 'Build failed before launch.');
  if (debug) {
    if (typeof project.run.debugProgram !== 'string') throw new PluginError('sample.target', 'This application has no native debug program.');
    return { program: posix.join(project.path, project.run.debugProgram), arguments: params.userArguments,
      workingDirectory: project.path, environment: {} };
  }
  return { executable: { kind: 'command', value: project.run.command },
    arguments: [...project.run.arguments, ...params.userArguments],
    workingDirectory: project.path, environment: {} };
}

async function build(selected, arguments_, context) {
  let output = '';
  for (const project of selected) {
    context.progress({ message: `Building ${project.name}` });
    const result = await run(project.build, project, arguments_, context);
    output += result.stdout + result.stderr;
    if (result.exitCode !== 0) return { exitCode: result.exitCode, output };
  }
  return { exitCode: 0, output };
}

async function run(command, project, arguments_, context) {
  context.throwIfCancelled();
  if (!Array.isArray(command) || command.length === 0 || !command.every((part) => typeof part === 'string')) {
    throw new PluginError('sample.command', 'A tool command must be a nonempty argument array.');
  }
  return context.run(command[0] === 'node' ? process.execPath : command[0], [...command.slice(1), ...arguments_], {
    cwd: await inside(project.path),
    onStdout: (output) => context.progress({ output }),
    onStderr: (output) => context.progress({ output }),
  });
}

async function select(params) {
  const all = await projects(params.root);
  if (!params.project) return all;
  const selected = all.filter(({ name, path }) => name === params.project.name && path === params.project.path);
  if (selected.length !== 1) throw new PluginError('sample.project', 'The selected project no longer exists.');
  return selected;
}

async function projects(root) {
  const base = await inside(root);
  const config = JSON.parse(await readFile(join(base, 'lithe-sample.json'), 'utf8'));
  if (!Array.isArray(config.projects)) throw new PluginError('sample.config', 'A projects array is required.');
  return Promise.all(config.projects.map(async (project) => {
    if (typeof project.path !== 'string' || isAbsolute(project.path)) throw new PluginError('sample.path', 'A project path must be relative to its instance.');
    const directory = await realpath(resolve(base, project.path));
    if (directory !== base && !directory.startsWith(base + sep)) throw new PluginError('sample.path', 'A project is outside its instance.');
    // The model is instance-relative; later request selectors are workspace-relative.
    const modelPath = relative(base, directory).split(sep).join('/') || '.';
    return { ...project, modelPath, path: posix.join(root, modelPath) };
  }));
}

async function inside(path) {
  if (typeof path !== 'string' || isAbsolute(path)) throw new PluginError('sample.path', 'A workspace-relative directory is required.');
  const directory = await realpath(resolve(workspace, path));
  if (directory !== workspace && !directory.startsWith(workspace + sep)) {
    throw new PluginError('sample.path', 'The directory is outside the workspace.');
  }
  return directory;
}

await backend.listen();
