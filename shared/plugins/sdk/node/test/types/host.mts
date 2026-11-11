// Check the host-service declarations the way a plugin uses them.
import { createBackend, HOST_ERRORS, HostError, type ContextReference, type JsonValue, type Subscription } from '@lithe/plugin-sdk';

const backend = createBackend();
const version: number | null = backend.functionsVersion;
void version;

backend.function('query.run', async (_params, context) => {
  const secret: string | null = await context.host.credentials.get('connection.prod');
  await context.host.credentials.set('connection.prod', secret ?? '');
  const workspace: ContextReference | null = context.workspaceContext;
  if (!workspace) return null;
  const { folders } = await context.host.workspace.getFolders(workspace);
  const root = folders[0];
  if (!root) return null;
  const file = await context.host.workspace.readFile(workspace, { resource: { rootID: root.rootID, path: 'README.md' }, maximumBytes: 4096 });
  const truncated: boolean = file.isTruncated;
  const { selectedIndex } = await context.host.window.showInformation(workspace, { text: 'Done', actions: ['Open'] });
  const choice: number | null = selectedIndex;
  return { truncated, choice };
});

backend.function('project.describe', async (_params, context) => {
  // The calling plugin of a relayed call, or null.
  const caller: { readonly pluginID: string } | null = context.caller;
  const callerID: string | undefined = caller?.pluginID;
  void callerID;
  if (caller) {
    // @ts-expect-error The caller is read-only.
    caller.pluginID = 'dev.example.other';
  }
  const workspace = context.workspaceContext;
  if (!workspace) return null;
  const { providers } = await context.host.functions.providers('languageImport.settings');
  const provider: { pluginID: string; version: number } | undefined = providers[0];
  if (!provider) return null;
  const { value } = await context.host.functions.invoke(workspace, {
    pluginID: provider.pluginID,
    functionID: 'languageImport.settings',
    params: { project: 'app' },
    timeoutMilliseconds: 5000,
    parentOperationID: 'op-1',
  }, { signal: new AbortController().signal });
  const result: JsonValue = value;
  // The minimum a call needs is the provider and the function.
  await backend.host.functions.invoke(workspace, { pluginID: provider.pluginID, functionID: 'languageImport.settings' });
  // @ts-expect-error A relayed call names the plugin that provides the function.
  await context.host.functions.invoke(workspace, { functionID: 'languageImport.settings' });
  // @ts-expect-error A relayed call's params are an object.
  await context.host.functions.invoke(workspace, { pluginID: provider.pluginID, functionID: 'languageImport.settings', params: 'x' });
  // @ts-expect-error A parent operation ID is a string.
  await context.host.functions.invoke(workspace, { pluginID: provider.pluginID, functionID: 'languageImport.settings', parentOperationID: 17 });
  // @ts-expect-error A relayed call takes the workspace context first.
  await context.host.functions.invoke({ pluginID: provider.pluginID, functionID: 'languageImport.settings' });
  // @ts-expect-error The providers of a function are asked by its function ID.
  await context.host.functions.providers({ functionID: 'languageImport.settings' });
  return result;
});

backend.onCommand('db.refresh', async ({ commandID, workspaceContext }) => {
  const id: string = commandID;
  void id;
  if (!workspaceContext) return;
  if (await backend.host.supports('lithe/host/workspace/watch')) {
    const subscription: Subscription = await backend.host.workspace.watch(
      workspaceContext,
      { resource: { rootID: 'r1', path: '' }, recursive: true },
      (event, { sequence, gap }) => {
        const next: number = sequence;
        const missed: boolean = gap;
        void event; void next; void missed;
      },
    );
    await subscription.dispose();
  }
});

backend.onContextsChanged(async ({ revision }) => {
  const { contexts } = await backend.host.workspace.listContexts();
  const authority: 'local' | 'remote' = contexts[0]!.authority.kind;
  void revision; void authority;
});

async function edit(context: ContextReference) {
  try {
    const result = await backend.host.editor.applyEdits(context, {
      documentID: 'd1',
      expectedVersion: 3,
      edits: [{ range: { start: { line: 1, column: 1 }, end: { line: 1, column: 1 } }, text: 'x' }],
    });
    const applied: boolean = result.applied;
    void applied;
  } catch (error) {
    if (error instanceof HostError && error.rpcCode === HOST_ERRORS['plugin.host.versionConflict']) {
      const details = error.details;
      void details;
    }
  }
  // @ts-expect-error An edit needs the version it was computed against.
  await backend.host.editor.applyEdits(context, { documentID: 'd1', edits: [] });
  // @ts-expect-error A credential secret is a string.
  await backend.host.credentials.set('key', 42);
}
void edit;

async function relayFailure(context: ContextReference) {
  const failedNumber: -32070 = HOST_ERRORS['plugin.function.failed'];
  const cycleNumber: -32065 = HOST_ERRORS['plugin.function.callCycle'];
  void failedNumber; void cycleNumber;
  try {
    await backend.host.functions.invoke(context, { pluginID: 'dev.example.go', functionID: 'languageImport.settings' });
  } catch (error) {
    if (error instanceof HostError && error.code === 'plugin.function.failed') {
      // The provider's own error code, set by the provider's function.
      const pluginErrorCode: JsonValue | undefined = error.details?.pluginErrorCode;
      void pluginErrorCode;
    }
  }
  // @ts-expect-error Only the codes in the contract are keys of HOST_ERRORS.
  void HOST_ERRORS['plugin.function.unknown'];
}
void relayFailure;

async function nativeTerminal(context: ContextReference) {
  const created = await backend.host.terminal.create(context, {
    executable: 'example-client', arguments: ['--interactive'], environment: { MODE: 'dev', OLD_MODE: null },
  });
  await backend.host.terminal.show(context, { terminalID: created.terminalID });
  const subscription = await backend.host.terminal.onExit(context, created.terminalID, (event, info) => {
    const code: number | null = event.exitCode;
    const reason: 'exited' | 'closed' = event.reason;
    void code; void reason; void info;
  });
  await subscription.dispose();
  await backend.host.terminal.close(context, { terminalID: created.terminalID, retainedContextBytes: 4096 });
  await backend.host.terminal.reopen(context, { terminalID: created.terminalID });
  await backend.host.terminal.dispose(context, { terminalID: created.terminalID });
}
void nativeTerminal;
