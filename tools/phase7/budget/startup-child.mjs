// Startup-only diagnostic. Never prepare a project or invoke compiler queries.
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

export async function loadReady(engine, directory, load, clock = process.hrtime.bigint) {
  if (!['go', 'rust'].includes(engine)) throw new TypeError('startup engine must be go or rust');
  const begin = clock();
  const processor = await load(directory);
  const duration = clock() - begin;
  if (duration <= 0n || typeof processor?.processLine !== 'function') {
    throw new Error('loader did not produce a live processor and positive duration');
  }
  return {processor, ready: {version: 1, kind: 'startup_ready', engine,
    load_processor_ns: String(duration)}};
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [engine, directory, ...extra] = process.argv.slice(2);
    if (!['go', 'rust'].includes(engine) || !directory || extra.length) {
      throw new Error('usage: startup-child.mjs go|rust ARTIFACT_DIRECTORY');
    }
    // Frontend import belongs to parent launch-to-ready, outside loadProcessor.
    const {loadProcessor} = await import(engine === 'go' ? './go/wasm.mjs' : './production-wasm.mjs');
    const {processor, ready} = await loadReady(engine, directory, loadProcessor);
    globalThis.phase7StartupProcessor = processor;
    process.stdout.write(JSON.stringify(ready) + '\n', error => {
      if (error) {process.stderr.write(String(error) + '\n'); process.exit(1);}
      // Go's suspended runtime remains live. Exit after the ready record flushes.
      process.exit(0);
    });
  } catch (error) {
    process.stderr.write(`wasm startup: ${error?.stack ?? error}\n`);
    process.exitCode = 1;
  }
}
