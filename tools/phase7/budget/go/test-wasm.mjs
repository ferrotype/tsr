import assert from 'node:assert/strict';
import test from 'node:test';
import {createProcessor, validateBuildRecord} from './wasm.mjs';
test('Go build modes require their exact reviewed contract',()=>{
 const current={version:1,mode:'phase7-budget',measurement_contract:2,performance_eligible:true,source_guard:true,build_exit:0};
 assert.doesNotThrow(()=>validateBuildRecord(current));
 assert.doesNotThrow(()=>validateBuildRecord({version:1,mode:'phase7-budget-prototype',performance_eligible:false}));
 for(const patch of [{version:2},{mode:'unknown'},{measurement_contract:1},{measurement_contract:undefined},{performance_eligible:false},{source_guard:false},{source_guard:undefined},{build_exit:1},{build_exit:undefined}])
  assert.throws(()=>validateBuildRecord({...current,...patch}),/build/);
 assert.throws(()=>validateBuildRecord({version:1,mode:'phase7-budget-prototype',performance_eligible:true}),/build/);
});
test('Go Node accounting collects at aligned stages and keeps native stats separate',()=>{
 const calls=[];const runtime={heap_alloc:'42'};
 const p=createProcessor(line=>{const r=JSON.parse(line);calls.push(r.command);return JSON.stringify({version:2,id:'1',kind:r.command==='checkpoint'?'checkpoint':r.command==='prepare'?'prepared':'executed',value:r.command==='checkpoint'?runtime:null});},()=>65536,()=>calls.push('gc'));
 p.processLine('{"version":1,"command":"prepare","id":"1"}');
 p.processLine('{"version":2,"command":"execute","id":"1"}');
 const r=JSON.parse(p.processLine('{"version":2,"command":"checkpoint","id":"1"}'));
 assert.deepEqual(calls,['prepare','gc','execute','checkpoint','gc']);assert.deepEqual(r.value.go_runtime,runtime);
 assert.equal(r.value.gc_policy,'v8_after_prepare_and_at_checkpoint');assert.equal(r.value.linear_memory_bytes,'65536');assert(BigInt(r.value.process_peak_rss_bytes)>0n);
});
test('Go Node API failures and traps are terminal and fresh processors remain independent',()=>{
 for(const error of [new WebAssembly.RuntimeError('unreachable'),new Error('API error')]) {
 const p=createProcessor(()=>{throw error;},()=>0,()=>{});assert.throws(()=>p.processLine('{}'),e=>e===error);assert.throws(()=>p.processLine('{}'),/failed/);
 }
 const p=createProcessor(()=>JSON.stringify({kind:'failure',value:'bad input'}),()=>0,()=>{});assert.equal(JSON.parse(p.processLine('{}')).kind,'failure');assert(p.failed);
});
