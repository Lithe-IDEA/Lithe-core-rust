// Runs the SDK end-to-end scenario in sdk/e2e against the Rust Core of this
// repository.
//
//   node shared/plugins/scripts/sdk-e2e.mjs
//
// The scenario files are Cargo integration tests of lithe-core, declared in
// rust/lithe-core/Cargo.toml behind the `plugin-sdk-e2e` feature, so a plain
// `cargo test` skips them. This script builds the sample backends and then
// runs those tests, which exercise the Node.js, Java and Rust SDKs, the native
// build workflows and shortcut import against Core at this working tree.
//
// The Node.js sample runs on the node running this script. The Java sample
// runs on LITHE_PLUGIN_SDK_JAVA_RUNTIME, or `java` on PATH, and is built with
// Maven unless LITHE_PLUGIN_SDK_JAVA_SAMPLE names a built sample-build.jar.
// The Rust sample backend is built with Cargo unless
// LITHE_PLUGIN_SDK_RUST_SAMPLE names a built sample-backend executable.
import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

/** shared/plugins: the SDKs, their samples and the contract. */
const plugins = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repository = resolve(plugins, '..', '..');
const tests = ['plugin_sdk_node', 'plugin_sdk_java', 'plugin_sdk_rust', 'plugin_sdk_build'];

function run(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { stdio: 'inherit', ...options });
    child.on('error', reject);
    child.on('close', (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} ${args.join(' ')} failed (${signal ?? `exit ${code}`})`));
    });
  });
}

function which(command) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn('sh', ['-c', 'command -v "$1"', 'sh', command],
      { stdio: ['ignore', 'pipe', 'inherit'], timeout: 15000, killSignal: 'SIGKILL' });
    let text = '';
    child.stdout.on('data', chunk => { text += chunk; });
    child.on('error', reject);
    child.on('close', code => code === 0 && text.trim()
      ? resolvePromise(text.trim())
      : reject(new Error(`${command} is required to run the SDK end-to-end check.`)));
  });
}

/** Builds both Java samples from the current SDK sources unless they are given. */
async function javaSamples() {
  const jar = process.env.LITHE_PLUGIN_SDK_JAVA_SAMPLE
    ? resolve(process.env.LITHE_PLUGIN_SDK_JAVA_SAMPLE)
    : join(plugins, 'sdk/java/examples/sample-build/target/sample-build.jar');
  const nativeJar = process.env.LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE
    ? resolve(process.env.LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE)
    : join(plugins, 'sdk/java/examples/sample-native-build/target/sample-native-build.jar');
  if (!process.env.LITHE_PLUGIN_SDK_JAVA_SAMPLE || !process.env.LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE) {
    // `clean` keeps Shade from reusing a sample JAR that bundles an older SDK
    // after only the reactor dependency changed.
    await run(await which('mvn'), ['-B', '-ntp', '-q', '-f', join(plugins, 'sdk/java/pom.xml'),
      '-DskipTests', 'clean', 'package']);
  }
  await access(jar);
  await access(nativeJar);
  return { jar, nativeJar };
}

async function rustSample() {
  if (process.env.LITHE_PLUGIN_SDK_RUST_SAMPLE) return resolve(process.env.LITHE_PLUGIN_SDK_RUST_SAMPLE);
  const manifest = join(plugins, 'sdk/rust/Cargo.toml');
  await run('cargo', ['build', '--locked', '--quiet', '--manifest-path', manifest, '-p', 'sample-backend']);
  const executable = join(plugins, 'sdk/rust/target/debug',
    process.platform === 'win32' ? 'sample-backend.exe' : 'sample-backend');
  await access(executable);
  return executable;
}

async function main() {
  const java = process.env.LITHE_PLUGIN_SDK_JAVA_RUNTIME || await which('java');
  const { jar, nativeJar } = await javaSamples();
  const rust = await rustSample();
  await run('cargo', ['test', '--locked', '--manifest-path', join(repository, 'rust/Cargo.toml'),
    '-p', 'lithe-core', '--features', 'plugin-sdk-e2e', ...tests.flatMap(test => ['--test', test])], {
    env: {
      ...process.env,
      LITHE_PLUGIN_SDK_ROOT: plugins,
      LITHE_PLUGIN_SDK_NODE: process.execPath,
      LITHE_PLUGIN_SDK_JAVA: java,
      LITHE_PLUGIN_SDK_JAVA_SAMPLE: jar,
      LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE: nativeJar,
      LITHE_PLUGIN_SDK_RUST_SAMPLE: rust,
    },
  });
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
