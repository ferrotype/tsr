#!/usr/bin/env node
// Tiny correctness witness only. No elapsed or budget acceptance calculation.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, mkdir, writeFile, realpath } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve, join, basename } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validateBuildRecord } from './wasm.mjs';

const args = process.argv.slice(2);
assert.equal(args.length, 4, 'usage: smoke.mjs --artifacts DIR --output NEW_DIR');
assert.equal(args[0], '--artifacts'); assert.equal(args[2], '--output');
const artifacts = await realpath(args[1]);
const output = resolve(args[3]);
const existingParent = await realpath(resolve(output, '..'));
assert(![...output.split('/'), ...existingParent.split('/')].some(p => p.toLowerCase() === 'target'), 'captures must survive target deletion');
await mkdir(output);
const sha = data => createHash('sha256').update(data).digest('hex');
const record = JSON.parse(await readFile(join(artifacts, 'build.json'), 'utf8'));
validateBuildRecord(record);
for (const [name, expected] of Object.entries(record.artifacts)) {
  const bytes = await readFile(join(artifacts, name));
  assert.equal(sha(bytes), expected.sha256); assert.equal(bytes.length, expected.bytes);
}
const bytes = text => [...new TextEncoder().encode(text)];
const prepare = {
  version: 1, command: 'prepare', id: '1', input: {
    current_directory: bytes('/project'), case_sensitive: true,
    entries: [
      {kind: 'physical_file', path: bytes('/project/tsconfig.json'), bytes: bytes('{"compilerOptions":{"strict":true,"outDir":"out"},"files":["main.ts"]}')},
      {kind: 'physical_file', path: bytes('/project/main.ts'), bytes: bytes('export const answer: number = 42;\n')},
    ],
    configuration: {config_path: bytes('/project/tsconfig.json'), arguments: []}, operation: 'check_emit',
  },
};
const commands = [prepare, ...['execute', 'checkpoint', 'observe', 'release'].map(command => ({version: 1, command, id: '1'}))];
const input = commands.map(c => JSON.stringify(c)).join('\n') + '\n';
await writeFile(join(output, 'commands.ndjson'), input, {flag: 'wx'});
const child = spawnSync(join(artifacts, 'budget-go'), [], {input, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024});
await writeFile(join(output, 'native.stdout.ndjson'), child.stdout ?? '', {flag: 'wx'});
await writeFile(join(output, 'native.stderr.txt'), child.stderr ?? '', {flag: 'wx'});
assert.equal(child.error, undefined); assert.equal(child.status, 0);
const native = child.stdout.trim().split('\n').map(JSON.parse);
assert.deepEqual(native.map(r => r.kind), ['prepared', 'executed', 'checkpoint', 'observation', 'released']);
const require = createRequire(import.meta.url);
require(join(artifacts, 'wasm_exec.cjs'));
const go = new globalThis.Go();
const wasm = await WebAssembly.instantiate(await readFile(join(artifacts, 'budget-go.wasm')), go.importObject);
const goExit = go.run(wasm.instance);
// main installs the function synchronously before the Go select parks.
assert.equal(typeof globalThis.phase7BudgetGo, 'function');
const jsRaw = commands.map(c => globalThis.phase7BudgetGo(JSON.stringify(c)));
const js = jsRaw.map(JSON.parse);
await writeFile(join(output, 'wasm.stdout.ndjson'), jsRaw.join('\n') + '\n', {flag: 'wx'});
assert.deepEqual(js.map(r => r.kind), native.map(r => r.kind));
assert.deepEqual(js[3].value, native[3].value, 'native/wasm production work differs');
assert.equal(js[3].value.checkers, '1');
assert.notEqual(js[3].value.types, '0');
assert.equal(js[3].value.diagnostics.length, 0);
assert.equal(js[3].value.writes.length, 1);
assert.equal(new TextDecoder().decode(Uint8Array.from(js[3].value.writes[0].name)), '/project/out/main.js');
const invalid = JSON.stringify({version: 2, command: 'prepare', id: '2', input: prepare.input});
const failed = JSON.parse(globalThis.phase7BudgetGo(invalid));
assert.equal(failed.kind, 'failure');
assert.equal(JSON.parse(globalThis.phase7BudgetGo(JSON.stringify(prepare))).kind, 'failure');
const nativeFailure = spawnSync(join(artifacts, 'budget-go'), [], {input: invalid + '\n', encoding: 'utf8'});
assert.equal(nativeFailure.status, 1); assert.equal(JSON.parse(nativeFailure.stdout).kind, 'failure');
await writeFile(join(output, 'terminal-controls.json'), JSON.stringify({wasm: failed, native_exit: nativeFailure.status, native_response: JSON.parse(nativeFailure.stdout)}, null, 2) + '\n', {flag: 'wx'});
const smokeSource = await readFile(fileURLToPath(import.meta.url));
await writeFile(join(output, basename(fileURLToPath(import.meta.url))), smokeSource, {flag: 'wx'});
await writeFile(join(output, 'result.json'), JSON.stringify({version: 1, performance_eligible: false, cases: ['native-wasm-production-equivalence', 'one-checker', 'optional-emit', 'terminal-invalid-version'], passed: true, node: process.version, pin: record.pin, build_record_sha256: sha(await readFile(join(artifacts, 'build.json'))), smoke_sha256: sha(smokeSource), observation_sha256: sha(Buffer.from(JSON.stringify(js[3].value)))}, null, 2) + '\n', {flag: 'wx'});
console.log(output);
void goExit;
