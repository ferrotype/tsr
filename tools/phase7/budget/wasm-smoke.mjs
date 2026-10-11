#!/usr/bin/env node
// Untimed equivalence witness. Neither observation JSON nor JS transport is a
// heap-budget denominator; checkpoints explicitly retain their own authority.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, realpath, copyFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { loadProcessor } from './production-wasm.mjs';
import { validateBuildRecord } from './go/wasm.mjs';

const args = process.argv.slice(2);
const streaming = args.at(-1) === '--stream'; if (streaming) args.pop();
assert.equal(args.length, 8, 'usage: wasm-smoke.mjs --go-artifacts DIR --rust-artifacts DIR --input INPUT.json --output NEW_DIR');
assert.deepEqual(args.filter((_, i) => i % 2 === 0), ['--go-artifacts', '--rust-artifacts', '--input', '--output']);
assert.equal(typeof globalThis.gc, 'function', 'run node --expose-gc');
const goDirectory = await realpath(args[1]), rustDirectory = await realpath(args[3]);
assert(!goDirectory.split('/').some(p => p.toLowerCase() === 'target'), 'Go artifacts must already be durable');
const output = resolve(args[7]);
const parent = await realpath(resolve(output, '..'));
assert(![...output.split('/'), ...parent.split('/')].some(p => p.toLowerCase() === 'target'), 'captures must survive target deletion');
await mkdir(output);
const sha = data => createHash('sha256').update(data).digest('hex');
const inputBytes = await readFile(args[5]);
await writeFile(join(output, 'input.json'), inputBytes, {flag: 'wx'});
const input = JSON.parse(inputBytes);
const commands = [];
const version = streaming ? 2 : 1;
const add = (command, extra = {}) => commands.push({version, command, id: '1', ...extra});
if (streaming) {
  const files = input.entries.filter(entry => entry.kind === 'physical_file');
  const b64 = bytes => Buffer.from(bytes).toString('base64');
  add('prepare_begin', {input: {...input, entries: []}, expected_entries: String(input.entries.length), expected_files: String(files.length), expected_contents: String(files.length)});
  add('prepare_metadata', {entries: input.entries.map(entry => ({kind: entry.kind, path_b64: b64(entry.path),
    ...(entry.kind === 'physical_file' ? {raw_size: String(entry.bytes.length)} : entry.kind === 'symlink' ? {target_b64: b64(entry.target)} : {})}))});
  for (const entry of files) {
    add('content_begin', {path_b64: b64(entry.path), raw_size: String(entry.bytes.length), sha256: sha(Buffer.from(entry.bytes))});
    for (let offset = 0; offset < entry.bytes.length; offset += 16) add('content_chunk', {offset: String(offset), bytes_b64: b64(entry.bytes.slice(offset, offset + 16))});
    add('content_end');
  }
  add('prepare_end');
} else add('prepare', {input});
for (const command of ['execute', 'checkpoint', 'observe', 'release']) add(command);
const wire = commands.map(JSON.stringify).join('\n') + '\n';
await writeFile(join(output, 'commands.ndjson'), wire, {flag: 'wx'});

// Persist the actual served production module once, before using it. The build
// record retains its original paths; loadProcessor resolves recorded basenames.
const alreadyDurable = !rustDirectory.split('/').some(p => p.toLowerCase() === 'target');
const retained = alreadyDurable ? rustDirectory : join(output, 'production-assets');
if (!alreadyDurable) await mkdir(retained);
const assets = {};
for (const name of ['build.json', 'checker_bg.wasm', 'bindings.mjs']) {
  const bytes = await readFile(join(rustDirectory, name));
  if (!alreadyDurable) await writeFile(join(retained, name), bytes, {flag: 'wx'});
  assets[name] = {sha256: sha(bytes), bytes: bytes.length};
}
const rust = await loadProcessor(retained);
const rustRaw = [];
try { for (const command of commands) rustRaw.push(rust.processLine(JSON.stringify(command))); }
finally { rust.dispose(); }
await writeFile(join(output, 'rust.stdout.ndjson'), rustRaw.join('\n') + '\n', {flag: 'wx'});

const goRecordBytes = await readFile(join(goDirectory, 'build.json'));
const goRecord = JSON.parse(goRecordBytes);
validateBuildRecord(goRecord);
for (const [name, expected] of Object.entries(goRecord.artifacts)) {
  const data = await readFile(join(goDirectory, name));
  assert.equal(sha(data), expected.sha256); assert.equal(data.length, expected.bytes);
}
const require = createRequire(import.meta.url); require(join(goDirectory, 'wasm_exec.cjs'));
const go = new globalThis.Go();
const wasm = await WebAssembly.instantiate(await readFile(join(goDirectory, 'budget-go.wasm')), go.importObject);
const exit = go.run(wasm.instance);
assert.equal(typeof globalThis.phase7BudgetGo, 'function');
const goRaw = commands.map(command => globalThis.phase7BudgetGo(JSON.stringify(command)));
await writeFile(join(output, 'go.stdout.ndjson'), goRaw.join('\n') + '\n', {flag: 'wx'});
const rustRows = rustRaw.map(JSON.parse).slice(-4), goRows = goRaw.map(JSON.parse).slice(-4);
for (const rows of [rustRows, goRows]) {
  assert.deepEqual(rows.map(row => row.kind), ['executed', 'checkpoint', 'observation', 'released']);
  assert(rows.every(row => row.version === version && row.id === '1'));
  assert.equal(rows[2].value.checkers, '1'); assert.notEqual(rows[2].value.types, '0');
  assert(rows[2].value.sources.length > 0);
}
function normalized(value) {
  const {types, options_json, ...rest} = value;
  const options = JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(Uint8Array.from(options_json)));
  if (options.paths && !Array.isArray(options.paths)) options.paths = Object.entries(options.paths);
  return {...rest, options};
}
assert.deepEqual(normalized(rustRows[2].value), normalized(goRows[2].value), 'production Go/Rust wasm work differs');
const invalid = JSON.stringify({version: 3, command: 'execute', id: '1'});
assert.equal(JSON.parse(globalThis.phase7BudgetGo(invalid)).kind, 'failure');
const freshRust = await loadProcessor(retained);
assert.throws(() => freshRust.processLine(invalid), /version/);
assert.throws(() => freshRust.processLine(wire.split('\n')[0]), /failed/); freshRust.dispose();
const child = spawnSync(resolve('tools/phase7/budget/production-wasm-consumer'), [], {
  input: wire, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024,
  env: {...process.env, TSR_WASM_ARTIFACT_DIR: retained},
});
await writeFile(join(output, 'child.stdout.ndjson'), child.stdout ?? '', {flag: 'wx'});
await writeFile(join(output, 'child.stderr.txt'), child.stderr ?? '', {flag: 'wx'});
assert.equal(child.error, undefined); assert.equal(child.status, 0);
assert.deepEqual(JSON.parse(child.stdout.trim().split('\n').at(-2)).value, rustRows[2].value);
const helpers = {};
for (const path of [fileURLToPath(import.meta.url), fileURLToPath(new URL('./production-wasm.mjs', import.meta.url)),
  fileURLToPath(new URL('./production-wasm-consumer', import.meta.url)), fileURLToPath(new URL('../../s10/wasm/instance.mjs', import.meta.url)),
  fileURLToPath(new URL('../../s10/wasm/imports.mjs', import.meta.url))]) {
  const name = path.split('/').at(-1); await copyFile(path, join(output, name));
  helpers[name] = sha(await readFile(path));
}
await writeFile(join(output, 'result.json'), JSON.stringify({version: 1, performance_eligible: false, passed: true,
  cases: ['production-go-rust-wasm-equivalence', 'one-checker', 'checkpoint-before-observation', 'terminal-invalid-version', 'child-transport'],
  node: process.version, input_sha256: sha(inputBytes), go_build_record_sha256: sha(goRecordBytes), go_artifact_directory: goDirectory,
  production_asset_directory: retained, production_assets: assets, helpers, protocol_version: version,
  go_types: goRows[2].value.types, rust_types: rustRows[2].value.types,
  checkpoint: {go: goRows[1].value, rust: rustRows[1].value}}, null, 2) + '\n', {flag: 'wx'});
console.log(output); void exit;
