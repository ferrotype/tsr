import assert from 'node:assert/strict';
import test from 'node:test';
import { createInstance, WasmApiError } from '../../../s10/wasm/instance.mjs';

const bytes = text => new TextEncoder().encode(text);
function mock(check = () => {}) {
  const calls = [];
  const raw = methods => ({free() {calls.push('free');}, __destroy_into_raw() {calls.push('detach');}, ...methods});
  const session = raw({check, emit_retained(json) {calls.push(['emit', json]);}, retained_snapshot() {return bytes('{"checkers":"1"}');}});
  const prepared = raw({compile_config(path, args) {calls.push(['config', [...path], args]);return session;}});
  class MemoryHost { free() {calls.push('host-free');} __destroy_into_raw() {calls.push('host-detach');} prepare() {calls.push('prepare');return prepared;} }
  const factory = () => ({initSync() {return {memory: {buffer: new ArrayBuffer(65536)}};}, MemoryHost});
  return {calls, instance: createInstance(factory, {})};
}
test('prepared config and retained stages keep observation outside execution', () => {
  const {instance, calls} = mock(() => calls.push('check'));
  const host = instance.createHost(bytes('/p'));
  const prepared = host.prepare();
  assert.throws(() => host.prepare(), /disposed/);
  assert.throws(() => prepared.compileConfig(bytes('/p/tsconfig.json'), ['bad']), TypeError);
  const session = prepared.compileConfig(bytes('/p/tsconfig.json'), [bytes('--strict')]);
  session.check(); session.emitRetained();
  assert.deepEqual(session.retainedSnapshot(), {checkers: '1'});
  assert.equal(instance.linearMemoryBytes, 65536);
  assert.deepEqual(calls.slice(0,4), ['prepare', ['config', [...bytes('/p/tsconfig.json')], JSON.stringify([[...bytes('--strict')]])], 'check', ['emit','{}']]);
  session.dispose(); instance.dispose();
  assert.equal(calls.filter(c => c === 'free').length, 1);
  assert.throws(() => instance.linearMemoryBytes, /disposed/);
});
test('retained-stage traps detach owners and forbid later instance entry', () => {
  const {instance, calls} = mock(() => {throw new WebAssembly.RuntimeError('unreachable');});
  const session = instance.createHost(bytes('/p')).prepare().compileConfig(bytes('/p/tsconfig.json'));
  assert.throws(() => session.check(), /unreachable/);
  assert.equal(instance.state, 'failed');
  assert(calls.includes('detach')); assert(!calls.includes('free'));
  assert.throws(() => instance.linearMemoryBytes, /failed/);
  assert.throws(() => session.retainedSnapshot(), /disposed/);
});
test('structured API errors keep independent instance resources live', () => {
  const {instance} = mock(() => {throw 'ts-wasm-api-error:retired';});
  const session = instance.createHost(bytes('/p')).prepare().compileConfig(bytes('/p/tsconfig.json'));
  assert.throws(() => session.check(), WasmApiError);
  assert.equal(instance.state, 'live');
  session.dispose(); instance.dispose();
});
