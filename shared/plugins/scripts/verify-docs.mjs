// Offline checks for the plugin developer resources in shared/plugins.
//
//   node shared/plugins/scripts/verify-docs.mjs
//
// - Every schema under contract/v1 names its own published URL as `$id`, so a
//   plugin's `$schema` reference resolves to the file in this repository.
// - Every relative Markdown link resolves to a file or directory in this
//   repository.
// - No link points at a file of the retired lithe-plugin-docs repository,
//   whose contents moved here. Links to its existing SDK releases stay valid.
import { readdir, readFile, stat } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative, resolve, sep } from 'node:path';

/** Where GitHub serves the contract; every schema's `$id` is its URL here. */
export const publishedContract = 'https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/';
const retired = /https:\/\/(?:github\.com\/Lithe-IDEA\/lithe-plugin-docs\/(?:blob|tree)\/|raw\.githubusercontent\.com\/Lithe-IDEA\/lithe-plugin-docs\/)/;
const skipped = new Set(['node_modules', 'target', '.artifacts']);

async function files(directory) {
  const found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (skipped.has(entry.name) || entry.name.startsWith('.')) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) found.push(...await files(path));
    else if (entry.isFile()) found.push(path);
  }
  return found;
}

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** Returns the problems found under `plugins`; `repository` bounds relative links. */
export async function verify(plugins, repository = resolve(plugins, '..', '..')) {
  const problems = [];
  const contract = join(plugins, 'contract', 'v1');
  for (const file of await files(plugins)) {
    const shown = relative(repository, file).split(sep).join('/');
    if (file.endsWith('.schema.json') && file.startsWith(contract + sep)) {
      const expected = publishedContract + relative(contract, file).split(sep).join('/');
      const id = JSON.parse(await readFile(file, 'utf8')).$id;
      if (id !== expected) problems.push(`${shown}: $id is ${JSON.stringify(id)}, expected ${JSON.stringify(expected)}`);
    }
    if (!file.endsWith('.md')) continue;
    const text = await readFile(file, 'utf8');
    for (const [, target] of text.matchAll(/\]\(([^)\s]+)\)/g)) {
      if (retired.test(target)) {
        problems.push(`${shown}: ${target} points at lithe-plugin-docs; link the file in this repository`);
        continue;
      }
      if (target.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
      const linked = resolve(dirname(file), decodeURIComponent(target.split('#')[0]));
      if (relative(repository, linked).startsWith('..')) {
        problems.push(`${shown}: ${target} leaves the repository; use an absolute URL`);
      } else if (!await exists(linked)) {
        problems.push(`${shown}: ${target} does not exist`);
      }
    }
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const problems = await verify(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
  for (const problem of problems) console.error(problem);
  if (problems.length) process.exitCode = 1;
  else console.log('Plugin docs and contract links are consistent.');
}
