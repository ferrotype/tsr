// Correctness transport over production tsr_wasm composable stages. The wire
// protocol lives here; production exports know nothing about the experiment.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';
import { createInstance } from '../../s10/wasm/instance.mjs';
import { checkImports } from '../../s10/wasm/imports.mjs';

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
function fields(value, names, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).length !== names.length || !names.every(name => Object.hasOwn(value, name))) {
    throw new TypeError(`${label}: missing or unknown fields`);
  }
}
function bytes(value, label) {
  if (!Array.isArray(value) || !value.every(v => Number.isInteger(v) && v >= 0 && v <= 255)) {
    throw new TypeError(`${label}: expected byte array`);
  }
  return Uint8Array.from(value);
}
function path(value, label) {
  bytes(value, label);
  if (value[0] !== 47 || value.includes(92) || (value.length > 1 && value.at(-1) === 47)) {
    throw new TypeError(`${label}: expected normalized absolute POSIX path`);
  }
  let start = 1;
  for (let i = 1; i <= value.length; i++) {
    if (i === value.length || value[i] === 47) {
      const length = i - start;
      if ((length === 0 && value.length !== 1) || (length === 1 && value[start] === 46) ||
          (length === 2 && value[start] === 46 && value[start + 1] === 46)) {
        throw new TypeError(`${label}: expected normalized absolute POSIX path`);
      }
      start = i + 1;
    }
  }
}
function parse(text) {
  const result = JSON.parse(text);
  // Reject duplicate JSON fields, including escaped spellings. JSON.parse
  // establishes syntax; this scan only follows its string/container tokens.
  const stack = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '{') stack.push(new Set());
    else if (text[i] === '[') stack.push(null);
    else if (text[i] === '}' || text[i] === ']') stack.pop();
    else if (text[i] === '"') {
      const start = i++;
      while (text[i] !== '"') { if (text[i] === '\\') i++; i++; }
      let next = i + 1;
      while (/\s/.test(text[next] ?? '')) next++;
      if (text[next] === ':') {
        const key = JSON.parse(text.slice(start, i + 1));
        if (stack.at(-1).has(key)) throw new TypeError(`duplicate field: ${key}`);
        stack.at(-1).add(key);
      }
    }
  }
  return result;
}
export function validateCommand(text) {
  const command = parse(text);
  if (command?.version === 2) return validateStreamCommand(command, text);
  const prepare = command?.command === 'prepare';
  fields(command, prepare ? ['version', 'command', 'id', 'input'] : ['version', 'command', 'id'], 'command');
  if (command.version !== 1) throw new TypeError('unsupported budget version');
  if (typeof command.id !== 'string' || !/^(0|[1-9][0-9]*)$/.test(command.id) || BigInt(command.id) > 0xffffffffffffffffn) {
    throw new TypeError('id must be canonical unsigned 64-bit decimal');
  }
  if (!['prepare', 'execute', 'checkpoint', 'observe', 'release'].includes(command.command)) throw new TypeError('unknown command');
  if (prepare) {
    const input = command.input;
    fields(input, ['current_directory', 'case_sensitive', 'entries', 'configuration', 'operation'], 'input');
    path(input.current_directory, 'current_directory');
    if (typeof input.case_sensitive !== 'boolean') throw new TypeError('case_sensitive must be boolean');
    if (!['check', 'check_emit'].includes(input.operation)) throw new TypeError('unsupported operation');
    fields(input.configuration, ['config_path', 'arguments'], 'configuration');
    path(input.configuration.config_path, 'config_path');
    if (!Array.isArray(input.configuration.arguments)) throw new TypeError('arguments must be array');
    input.configuration.arguments.forEach(arg => bytes(arg, 'argument'));
    if (!Array.isArray(input.entries)) throw new TypeError('entries must be array');
    for (const entry of input.entries) {
      const extra = {directory: [], physical_file: ['bytes'], symlink: ['target']}[entry?.kind];
      if (!extra) throw new TypeError('unsupported entry kind');
      fields(entry, ['kind', 'path', ...extra], 'entry');
      path(entry.path, 'entry.path');
      extra.forEach(key => bytes(entry[key], `entry.${key}`));
    }
  }
  return command;
}
function decimal(value, label) {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value) || BigInt(value) > 0xffffffffffffffffn) {
    throw new TypeError(`${label}: expected canonical unsigned 64-bit decimal`);
  }
  return BigInt(value);
}
function base64(value, label) {
  if (typeof value !== 'string') throw new TypeError(`${label}: expected base64`);
  const data = Buffer.from(value, 'base64');
  if (data.toString('base64') !== value) throw new TypeError(`${label}: expected canonical base64`);
  return data;
}
function validateStreamCommand(command, text) {
  if (Buffer.byteLength(text) > 2 * 1024 * 1024) throw new RangeError('v2 frame exceeds 2 MiB');
  const extra = {prepare_begin: ['input', 'expected_entries', 'expected_files', 'expected_contents'],
    prepare_metadata: ['entries'], content_begin: ['path_b64', 'raw_size', 'sha256'],
    content_chunk: ['offset', 'bytes_b64'], content_end: [], prepare_end: [],
    execute: [], checkpoint: [], observe: [], observe_begin: [], observe_source: ['index'], observe_write: ['index'], release: []}[command.command];
  if (!extra) throw new TypeError('unknown v2 command');
  fields(command, ['version', 'command', 'id', ...extra], 'v2 command'); decimal(command.id, 'id');
  if (command.command === 'prepare_begin') {
    validateCommand(JSON.stringify({version: 1, command: 'prepare', id: command.id, input: command.input}));
    if (command.input.entries.length !== 0) throw new TypeError('prepare_begin requires empty entries');
    for (const name of ['expected_entries', 'expected_files', 'expected_contents']) decimal(command[name], name);
  } else if (command.command === 'prepare_metadata') {
    if (!Array.isArray(command.entries) || Buffer.byteLength(JSON.stringify(command.entries)) > 1024 * 1024) throw new RangeError('metadata array exceeds 1 MiB');
    for (const entry of command.entries) {
      const extra = {directory: [], physical_file: ['raw_size'], symlink: ['target_b64']}[entry?.kind];
      if (!extra) throw new TypeError('unknown metadata entry');
      fields(entry, ['kind', 'path_b64', ...extra], 'metadata entry');
      path([...base64(entry.path_b64, 'path_b64')], 'metadata path');
      if (entry.kind === 'physical_file') decimal(entry.raw_size, 'raw_size');
      if (entry.kind === 'symlink') base64(entry.target_b64, 'target_b64');
    }
  } else if (command.command === 'content_begin') {
    path([...base64(command.path_b64, 'path_b64')], 'content path'); decimal(command.raw_size, 'raw_size');
    if (typeof command.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(command.sha256)) throw new TypeError('invalid sha256');
  } else if (command.command === 'content_chunk') {
    decimal(command.offset, 'offset');
    if (base64(command.bytes_b64, 'bytes_b64').length > 1024 * 1024) throw new RangeError('decoded chunk exceeds 1 MiB');
  }
  if (command.command === 'observe_source' || command.command === 'observe_write') decimal(command.index, 'index');
  return command;
}

export function createProcessor(instance, collect = globalThis.gc) {
  let state = 'empty', id, input, prepared, session, protocol, host, stream;
  function requireState(expected, command) {
    if (state !== expected || command.id !== id) throw new Error(`${command.command} requires ${expected} operation with matching id`);
  }
  return Object.freeze({
    processLine(text) {
      if (state === 'failed') throw new Error('budget consumer is failed');
      try {
        const command = validateCommand(text);
        let kind, value = null;
        if (state !== 'empty' && command.version !== protocol) throw new Error('cannot mix protocol versions');
        if (command.version === 2 && !['execute', 'checkpoint', 'observe', 'observe_begin', 'observe_source', 'observe_write', 'release'].includes(command.command)) {
          kind = prepareStream(command);
          if (kind === 'prepared') {
            if (typeof collect !== 'function') throw new Error('prepare requires node --expose-gc');
            collect();
          }
          return JSON.stringify({version: 2, id: command.id, kind, value});
        }
        switch (command.command) {
          case 'prepare': {
            if (state !== 'empty') throw new Error('prepare requires empty consumer');
            input = command.input; id = command.id; protocol = 1;
            const host = instance.createHost(bytes(input.current_directory), input.case_sensitive);
            for (const entry of input.entries) {
              const path = bytes(entry.path);
              if (entry.kind === 'directory') host.addDirectory(path);
              else if (entry.kind === 'physical_file') host.addFile(path, bytes(entry.bytes));
              else host.addSymlink(path, bytes(entry.target));
            }
            prepared = host.prepare();
            // The wasm snapshot owns the physical bytes. Retain only operation
            // configuration, not the decoded transport's duplicate payload.
            input.entries = undefined;
            state = 'prepared'; kind = 'prepared';
            if (typeof collect !== 'function') throw new Error('prepare requires node --expose-gc');
            collect();
            break;
          }
          case 'execute':
            requireState('prepared', command);
            session = prepared.compileConfig(bytes(input.configuration.config_path), input.configuration.arguments.map(arg => bytes(arg)));
            prepared = undefined;
            session.check();
            if (input.operation === 'check_emit') session.emitRetained();
            state = 'retained'; kind = 'executed';
            break;
          case 'checkpoint': {
            requireState('retained', command);
            if (typeof collect !== 'function') throw new Error('checkpoint requires node --expose-gc');
            collect();
            const memory = process.memoryUsage();
            value = {authority: 'production-wasm-node', linear_memory_bytes: String(instance.linearMemoryBytes),
              process_rss_bytes: String(memory.rss), v8_heap_used_bytes: String(memory.heapUsed),
              external_bytes: String(memory.external), array_buffer_bytes: String(memory.arrayBuffers),
              process_peak_rss_bytes: String(process.resourceUsage().maxRSS * 1024),
              gc_policy: 'v8_after_prepare_and_at_checkpoint'};
            state = 'checkpointed'; kind = 'checkpoint';
            break;
          }
          case 'observe':
            requireState('checkpointed', command);
            value = session.retainedSnapshot(); kind = 'observation';
            break;
          case 'observe_begin':
            requireState('checkpointed', command);
            value = session.retainedSnapshotHeader(); kind = 'observation_begin';
            break;
          case 'observe_source':
          case 'observe_write': {
            requireState('checkpointed', command);
            const index = decimal(command.index, 'index');
            if (index > 0xffffffffn) throw new RangeError('wasm row index exceeds unsigned 32-bit range');
            value = command.command === 'observe_source' ? session.retainedSource(Number(index)) : session.retainedWrite(Number(index));
            kind = command.command === 'observe_source' ? 'observation_source' : 'observation_write';
            break;
          }
          case 'release':
            requireState('checkpointed', command);
            session.dispose(); session = undefined; input = undefined; id = undefined; protocol = undefined;
            state = 'empty'; kind = 'released';
            break;
        }
        return JSON.stringify({version: command.version, id: command.id, kind, value});
      } catch (error) {
        state = 'failed';
        throw error;
      }
    },
    dispose() { instance.dispose(); input = prepared = session = host = stream = undefined; state = 'failed'; },
    get state() { return state; },
  });
  function key(path) {
    // Case-insensitive non-ASCII paths need the compiler's own canonicalizer.
    // This first adapter refuses that shape rather than guessing Unicode rules.
    if (!input.case_sensitive && path.some(byte => byte > 127)) throw new TypeError('case-insensitive non-ASCII metadata path unsupported');
    return Buffer.from(input.case_sensitive ? path : path.map(byte => byte >= 65 && byte <= 90 ? byte + 32 : byte)).toString('hex');
  }
  function prepareStream(command) {
    if (command.command === 'prepare_begin') {
      if (state !== 'empty') throw new Error('prepare_begin requires empty consumer');
      const expectedEntries = decimal(command.expected_entries), expectedFiles = decimal(command.expected_files), expectedContents = decimal(command.expected_contents);
      if (expectedFiles > expectedEntries || expectedContents > expectedFiles) throw new Error('invalid declared preparation counts');
      input = command.input; id = command.id; protocol = 2;
      host = instance.createHost(bytes(input.current_directory), input.case_sensitive);
      stream = {expectedEntries, expectedFiles, expectedContents, entries: 0n, files: 0n, contents: 0n, paths: new Map(), loaded: new Set(), contentStarted: false};
      state = 'preparing'; return 'preparing';
    }
    requireState('preparing', command);
    if (command.command === 'prepare_metadata') {
      if (stream.contentStarted) throw new Error('metadata cannot follow content');
      for (const entry of command.entries) {
        const path = base64(entry.path_b64), canonical = key(path);
        if (stream.paths.has(canonical)) throw new Error('duplicate canonical metadata path');
        if (entry.kind === 'directory') host.addDirectory(path);
        else if (entry.kind === 'symlink') host.addSymlink(path, base64(entry.target_b64));
        else { host.addUnavailableFile(path, BigInt(entry.raw_size)); stream.files++; }
        stream.paths.set(canonical, {kind: entry.kind, path, size: entry.raw_size}); stream.entries++;
        if (stream.entries > stream.expectedEntries || stream.files > stream.expectedFiles) throw new Error('metadata exceeds declared counts');
      }
      return 'metadata';
    }
    if (command.command === 'content_begin') {
      if (stream.content || stream.entries !== stream.expectedEntries || stream.files !== stream.expectedFiles) throw new Error('content requires complete metadata and no open file');
      const canonical = key(base64(command.path_b64)), entry = stream.paths.get(canonical);
      if (!entry || entry.kind !== 'physical_file' || stream.loaded.has(canonical)) throw new Error('content requires unique known physical placeholder');
      if (entry.size !== command.raw_size) throw new Error('content size differs from metadata');
      const size = BigInt(command.raw_size);
      if (size > 2147483647n) throw new RangeError('physical content exceeds signed source positions');
      stream.content = {canonical, path: entry.path, data: Buffer.alloc(Number(size)), offset: 0, hash: createHash('sha256'), expectedHash: command.sha256};
      stream.contentStarted = true; return 'content_started';
    }
    if (command.command === 'content_chunk') {
      const content = stream.content;
      if (!content || BigInt(content.offset) !== BigInt(command.offset)) throw new Error('noncontiguous content offset');
      const chunk = base64(command.bytes_b64);
      if (chunk.length > content.data.length - content.offset) throw new Error('content exceeds metadata size');
      content.data.set(chunk, content.offset); content.offset += chunk.length; content.hash.update(chunk);
      return 'content_chunk';
    }
    if (command.command === 'content_end') {
      const content = stream.content;
      if (!content || content.offset !== content.data.length || content.hash.digest('hex') !== content.expectedHash) throw new Error('content byte count or hash mismatch');
      host.addFile(content.path, content.data); stream.loaded.add(content.canonical); stream.contents++; stream.content = undefined;
      if (stream.contents > stream.expectedContents) throw new Error('content exceeds declared count');
      return 'content_verified';
    }
    if (stream.content || stream.entries !== stream.expectedEntries || stream.files !== stream.expectedFiles || stream.contents !== stream.expectedContents) throw new Error('incomplete streamed snapshot');
    prepared = host.prepare(); host = stream = undefined; input.entries = undefined; state = 'prepared'; return 'prepared';
  }
}

export async function loadProcessor(directory) {
  if (typeof directory !== 'string' || directory.length === 0) throw new Error('TSR_WASM_ARTIFACT_DIR must name a production checker build');
  directory = resolve(directory);
  const record = JSON.parse(await readFile(resolve(directory, 'build.json'), 'utf8'));
  if (record.mode !== 'checker' || !Array.isArray(record.command) ||
      !record.command.some((part, i) => part === '-p' && record.command[i + 1] === 'tsr_wasm') ||
      !record.command.some((part, i) => part === '--features' && record.command[i + 1].split(',').includes('checker'))) {
    throw new Error('only the production tsr_wasm checker artifact is eligible');
  }
  const files = new Map();
  for (const name of ['checker_bg.wasm', 'bindings.mjs']) {
    const data = await readFile(resolve(directory, name));
    const entries = Object.entries(record.artifacts ?? {}).filter(([path]) => path.split('/').at(-1) === name);
    if (entries.length !== 1 || entries[0][1].sha256 !== sha(data) || entries[0][1].bytes !== data.length) {
      throw new Error(`artifact differs from build record: ${name}`);
    }
    files.set(name, data);
  }
  const module = new WebAssembly.Module(files.get('checker_bg.wasm'));
  checkImports(module, 'checker');
  const names = new Set(WebAssembly.Module.exports(module).map(item => item.name));
  for (const name of ['memoryhost_prepare', 'preparedhost_compile_config', 'wasmsession_check', 'wasmsession_emit_retained', 'wasmsession_retained_snapshot', 'wasmsession_retained_snapshot_header', 'wasmsession_retained_source', 'wasmsession_retained_write']) {
    if (!names.has(name)) throw new Error(`production export is missing: ${name}`);
  }
  const { createBindings } = await import(pathToFileURL(resolve(directory, 'bindings.mjs')));
  return createProcessor(createInstance(createBindings, module));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  let processor, reader;
  try {
    if (process.argv.length !== 2) throw new Error('consumer takes no arguments; set TSR_WASM_ARTIFACT_DIR');
    processor = await loadProcessor(process.env.TSR_WASM_ARTIFACT_DIR);
    reader = createInterface({input: process.stdin, crlfDelay: Infinity});
    for await (const line of reader) {
      const response = processor.processLine(line);
      await new Promise((accept, reject) => process.stdout.write(response + '\n', error => error ? reject(error) : accept()));
    }
  } catch (error) {
    process.stderr.write(`production wasm budget consumer: ${error?.stack ?? error}\n`);
    process.exitCode = 1;
  } finally {
    reader?.close();
    process.stdin.destroy();
    processor?.dispose();
  }
}
