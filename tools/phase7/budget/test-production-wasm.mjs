import assert from 'node:assert/strict';
import test from 'node:test';
import {createHash} from 'node:crypto';
import { createProcessor, validateCommand, loadProcessor } from './production-wasm.mjs';

const byte = text => [...new TextEncoder().encode(text)];
const input = () => ({current_directory: byte('/p'), case_sensitive: true, entries: [],
  configuration: {config_path: byte('/p/tsconfig.json'), arguments: []}, operation: 'check_emit'});
const command = (name, value) => JSON.stringify({version: 1, command: name, id: '18446744073709551615', ...(value ? {input: value} : {})});
function mock(check) {
  const calls = [];
  const session = {check: () => {calls.push('check'); check?.();}, emitRetained: () => calls.push('emit'),
    retainedSnapshot: () => {calls.push('observe'); return {checkers: '1', sources: [{text:[255,0]}], writes:[{text:[0,254]}]};}, retainedSnapshotHeader: () => {calls.push('header');return {checkers:'1',source_count:'1',write_count:'1'};},
    retainedSource: index => {calls.push(['source',index]);return {text:[255,0]};},
    retainedWrite: index => {calls.push(['write',index]);return {text:[0,254]};}, dispose: () => calls.push('release')};
  const instance = {createHost: () => ({addFile: () => calls.push('file'), addUnavailableFile: () => calls.push('unavailable'),
    addDirectory: () => {}, addSymlink: () => {}, prepare: () => {calls.push('prepare'); return {compileConfig: () => {calls.push('config'); return session;}};}}),
    linearMemoryBytes: 65536, dispose: () => calls.push('dispose')};
  return {calls, processor: createProcessor(instance, () => calls.push('gc'))};
}
test('stages retain results through checkpoint before any observation', () => {
  const {calls, processor} = mock();
  for (const name of ['prepare', 'execute', 'checkpoint', 'observe', 'release']) {
    const result = JSON.parse(processor.processLine(command(name, name === 'prepare' ? input() : undefined)));
    assert.equal(result.id, '18446744073709551615');
    if (name === 'checkpoint') {
      assert.equal(result.value.authority, 'production-wasm-node');
      assert.equal(result.value.linear_memory_bytes, '65536');
      assert(!calls.includes('observe'));
      assert.equal(result.value.gc_policy,'v8_after_prepare_and_at_checkpoint');
      assert(BigInt(result.value.process_peak_rss_bytes)>0n);
    }
  }
  assert.deepEqual(calls, ['prepare', 'gc', 'config', 'check', 'emit', 'gc', 'observe', 'release']);
  assert.equal(processor.state, 'empty'); processor.dispose();
});
function streamCommands(hash) {
  const path = Buffer.from('/p/tsconfig.json').toString('base64');
  const bytes = Buffer.from('{}');
  return [
    ['prepare_begin', {input: input(), expected_entries: '1', expected_files: '1', expected_contents: '1'}],
    ['prepare_metadata', {entries: [{kind: 'physical_file', path_b64: path, raw_size: '2'}]}],
    ['content_begin', {path_b64: path, raw_size: '2', sha256: hash ?? createHash('sha256').update(bytes).digest('hex')}],
    ['content_chunk', {offset: '0', bytes_b64: bytes.toString('base64')}], ['content_end', {}], ['prepare_end', {}],
  ].map(([command, extra]) => JSON.stringify({version: 2, command, id: '1', ...extra}));
}
test('stream verifies physical bytes before freezing and compiler stages', () => {
  const {processor, calls} = mock();
  const responses = streamCommands().map(line => JSON.parse(processor.processLine(line)));
  assert.deepEqual(responses.map(row => row.kind), ['preparing', 'metadata', 'content_started', 'content_chunk', 'content_verified', 'prepared']);
  assert.deepEqual(calls, ['unavailable', 'file', 'prepare', 'gc']);
  processor.processLine('{"version":2,"command":"execute","id":"1"}');
  assert.deepEqual(calls.slice(-3), ['config', 'check', 'emit']); processor.dispose();
});
test('stream refuses wrong content hash, noncontiguous offset and duplicate canonical paths', () => {
  for (const variation of ['hash', 'offset', 'duplicate']) {
    const {processor, calls} = mock(); const commands = streamCommands(variation === 'hash' ? '0'.repeat(64) : undefined);
    if (variation === 'offset') commands[3] = commands[3].replace('"offset":"0"', '"offset":"1"');
    if (variation === 'duplicate') commands.splice(2, 0, commands[1]);
    assert.throws(() => commands.forEach(line => processor.processLine(line)));
    assert.equal(processor.state, 'failed'); assert(!calls.includes('prepare')); assert(!calls.includes('check')); processor.dispose();
  }
});
test('wrong order, API failure and trap make transport terminal', () => {
  for (const failure of [new Error('API failure'), new WebAssembly.RuntimeError('unreachable')]) {
    const {processor} = mock(() => {throw failure;});
    processor.processLine(command('prepare', input()));
    assert.throws(() => processor.processLine(command('execute')), error => error === failure);
    assert.equal(processor.state, 'failed');
    assert.throws(() => processor.processLine(command('prepare', input())), /failed/);
    processor.dispose();
    const fresh = mock().processor;
    fresh.processLine(command('prepare', input())); fresh.processLine(command('execute'));
    fresh.dispose();
  }
  const {processor} = mock();
  processor.processLine(command('prepare', input())); processor.processLine(command('execute'));
  assert.throws(() => processor.processLine(command('observe')), /checkpointed/);
  processor.dispose();
});
test('wire refuses ambiguous fields, null bytes and noncanonical identities', async () => {
  const good = command('prepare', input());
  for (const raw of [good.replace('"version":1', '"version":1,"version":1'),
    good.replace('"version":1', '"version":1,"Version":1'), good.replace('18446744073709551615', '18446744073709551616'),
    good.replace('"current_directory":[47,112]', '"current_directory":[null]'), good.replace('"current_directory":[47,112]', '"current_directory":[47,46,47,112]'), good.replace('"entries":[]', '"entries":null'),
    command('execute').replace('"version":1', '"version":3')]) assert.throws(() => validateCommand(raw));
  assert.equal(validateCommand(good).id, '18446744073709551615');
  await assert.rejects(loadProcessor(undefined), /TSR_WASM_ARTIFACT_DIR/);
});

test('v2 row commands preserve bytes and validate indices without full snapshot JSON',()=>{
 const {processor,calls}=mock();streamCommands().forEach(line=>processor.processLine(line));
 for(const command of ['execute','checkpoint'])processor.processLine(JSON.stringify({version:2,command,id:'1'}));
 const row=(command,index)=>JSON.parse(processor.processLine(JSON.stringify({version:2,command,id:'1',...(index===undefined?{}:{index})})));
 assert.equal(row('observe_begin').value.source_count,'1');
 assert.deepEqual(row('observe_source','0').value,{text:[255,0]});
 assert.deepEqual(row('observe_write','0').value,{text:[0,254]});
 assert(!calls.includes('observe'));assert.equal(calls.filter(c=>c==='gc').length,2);
 assert.throws(()=>row('observe_source','4294967296'),/32-bit/);processor.dispose();
 for(const index of ['01',1,null,'18446744073709551616'])assert.throws(()=>validateCommand(JSON.stringify({version:2,command:'observe_source',id:'1',index})));
});
