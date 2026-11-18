// Check the shortcut import declarations the way a plugin uses them.
import { createBackend } from '@lithe/plugin-sdk';

createBackend().keymapImportProvider('example', async (snapshot, context) => {
  context.throwIfCancelled();
  const files = await snapshot.listInputs();
  const bytes: Uint8Array = await snapshot.readChunk(files[0].id, 0, 1024);
  return { bindings: bytes.length ? [{ commandID: 'find-next', keys: [{ key: 'f3', modifiers: [] }] }] : [] };
});
