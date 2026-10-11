// NDJSON frontend over the same pinned Go body as the native consumer.
import {createHash} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createInterface} from 'node:readline';

export function validateBuildRecord(record) {
  if (record?.version !== 1) throw new Error('unsupported Go budget build version');
  if (record.mode === 'phase7-budget-prototype' && record.performance_eligible === false) return;
  if (record.mode === 'phase7-budget' && record.measurement_contract === 2 &&
      record.performance_eligible === true && record.source_guard === true && record.build_exit === 0) return;
  throw new Error('not a verified Go budget build');
}

export async function loadProcessor(directory) {
  if (typeof directory !== 'string' || directory.length === 0) throw new Error('TSR_GO_WASM_ARTIFACT_DIR must name the pinned Go budget build');
  directory = resolve(directory);
  const record = JSON.parse(await readFile(resolve(directory, 'build.json'), 'utf8'));
  validateBuildRecord(record);
  for (const name of ['budget-go.wasm', 'wasm_exec.cjs']) {
    const bytes = await readFile(resolve(directory, name));
    const entry = record.artifacts?.[name];
    if (entry?.sha256 !== createHash('sha256').update(bytes).digest('hex') || entry.bytes !== bytes.length) throw new Error(`Go artifact differs: ${name}`);
  }
  createRequire(import.meta.url)(resolve(directory, 'wasm_exec.cjs'));
  const go = new globalThis.Go();
  const wasm = await WebAssembly.instantiate(await readFile(resolve(directory, 'budget-go.wasm')), go.importObject);
  let runtimeFailed = false;
  void go.run(wasm.instance).catch(error => {runtimeFailed = true; process.stderr.write(`Go runtime terminated: ${error}\n`);});
  if (typeof globalThis.phase7BudgetGo !== 'function') throw new Error('Go body did not install its synchronous frontend');
  return createProcessor(line => globalThis.phase7BudgetGo(line), () => wasm.instance.exports.mem.buffer.byteLength, globalThis.gc, () => runtimeFailed);
}
// The native/wasm Go compiler body is unchanged by outer Node accounting.
export function createProcessor(invoke, linearBytes, collect = globalThis.gc, runtimeFailed = () => false) {
  let failed = false;
  return Object.freeze({
    processLine(line) {
      if (failed || runtimeFailed()) throw new Error('Go wasm consumer is failed');
      try {
        if (Buffer.byteLength(line) > 256 * 1024 * 1024) throw new RangeError('Go budget frame exceeds 256 MiB');
        const response = invoke(line);
        if (typeof response !== 'string') throw new TypeError('Go body returned a non-JSON response');
        const value = JSON.parse(response);
        if (value.kind === 'failure') failed = true;
        if (value.kind === 'prepared') {
          if (typeof collect !== 'function') throw new Error('prepare requires node --expose-gc');
          collect();
        }
        if (value.kind === 'checkpoint') {
          // Keep Go runtime bytes and Node/wasm capacity as distinct facts.
          // This outer envelope is not accepted as native Go HeapAlloc.
          if (typeof collect !== 'function') throw new Error('checkpoint requires node --expose-gc');
          collect();
          const memory = process.memoryUsage();
          value.value = {authority: 'go-wasm-node', go_runtime: value.value,
            linear_memory_bytes: String(linearBytes()),
            process_rss_bytes: String(memory.rss), v8_heap_used_bytes: String(memory.heapUsed),
            external_bytes: String(memory.external), array_buffer_bytes: String(memory.arrayBuffers),
            process_peak_rss_bytes: String(process.resourceUsage().maxRSS * 1024),
            gc_policy: 'v8_after_prepare_and_at_checkpoint'};
          return JSON.stringify(value);
        }
        return response;
      } catch (error) { failed = true; throw error; }
    },
    get failed() {return failed;},
  });
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  let reader;
  try {
    if (process.argv.length !== 2) throw new Error('consumer takes no arguments; set TSR_GO_WASM_ARTIFACT_DIR');
    const processor = await loadProcessor(process.env.TSR_GO_WASM_ARTIFACT_DIR);
    reader = createInterface({input: process.stdin, crlfDelay: Infinity});
    for await (const line of reader) {
      const response = processor.processLine(line);
      await new Promise((accept,reject) => process.stdout.write(response + '\n', error => error ? reject(error) : accept()));
      if (processor.failed) {process.exitCode = 1; break;}
    }
  } catch (error) {process.stderr.write(`Go wasm budget consumer: ${error?.stack ?? error}\n`); process.exitCode = 1;}
  finally {reader?.close(); process.stdin.destroy();}
}
