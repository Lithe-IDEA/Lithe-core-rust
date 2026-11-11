import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { publishedContract, verify } from './verify-docs.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'plugin-docs-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const plugins = join(root, 'shared', 'plugins');
  await mkdir(join(plugins, 'contract', 'v1'), { recursive: true });
  await mkdir(join(plugins, 'guides'));
  await writeFile(join(root, 'README.md'), '# Root\n');
  return { root, plugins };
}

test('consistent schemas and links pass', async (t) => {
  const { root, plugins } = await fixture(t);
  await writeFile(join(plugins, 'contract/v1/a.schema.json'),
    JSON.stringify({ $id: `${publishedContract}a.schema.json` }));
  await writeFile(join(plugins, 'guides/start.md'),
    '[contract](../contract/v1/a.schema.json) [root](../../../README.md#root) [web](https://example.com) [anchor](#top)\n'
    + '[old release](https://github.com/Lithe-IDEA/lithe-plugin-docs/releases/tag/sdk-node-v0.4.0)\n');
  assert.deepEqual(await verify(plugins, root), []);
});

test('a wrong $id, a missing target, an escaping link and a retired repository link are reported', async (t) => {
  const { root, plugins } = await fixture(t);
  await writeFile(join(plugins, 'contract/v1/a.schema.json'), JSON.stringify({ $id: 'https://example.com/a.schema.json' }));
  await writeFile(join(plugins, 'guides/start.md'),
    '[missing](missing.md) [outside](../../../../elsewhere.md)\n'
    + '[moved](https://github.com/Lithe-IDEA/lithe-plugin-docs/blob/main/guides/start.md)\n');
  const problems = await verify(plugins, root);
  assert.equal(problems.length, 4, problems.join('\n'));
  assert.match(problems.join('\n'), /\$id is/);
  assert.match(problems.join('\n'), /missing\.md does not exist/);
  assert.match(problems.join('\n'), /leaves the repository/);
  assert.match(problems.join('\n'), /points at lithe-plugin-docs/);
});
