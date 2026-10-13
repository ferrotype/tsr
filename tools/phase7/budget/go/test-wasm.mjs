import assert from 'node:assert/strict';
import test from 'node:test';
import {createHash} from 'node:crypto';
import {mkdtemp, readFile, writeFile, rm} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {tmpdir} from 'node:os';
import {join, basename} from 'node:path';
import {createProcessor, loadProcessor, validateBuildRecord} from './wasm.mjs';
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

test('Go loader executes and compiles the authenticated buffers once, bypassing require cache', async()=>{
 const directory=await mkdtemp(join(tmpdir(),'phase7-go-loader-'));
 const runtimePath=join(directory,'wasm_exec.cjs');
 const require=createRequire(import.meta.url);
 try {
  await writeFile(runtimePath,'globalThis.phase7RuntimeSource="cached";');
  require(runtimePath);
  const wasm=Buffer.from([0,97,115,109,1,0,0,0]);
  const runtime=Buffer.from(`globalThis.phase7RuntimeSource="verified";
   globalThis.Go=class {constructor(){this.importObject={};} run(){
    globalThis.phase7BudgetGo=()=>'{"kind":"executed"}'; return new Promise(()=>{});
   }};`);
  await writeFile(runtimePath,runtime);
  await writeFile(join(directory,'budget-go.wasm'),wasm);
  const entry=data=>({bytes:data.length,sha256:createHash('sha256').update(data).digest('hex')});
  await writeFile(join(directory,'build.json'),JSON.stringify({version:1,mode:'phase7-budget-prototype',performance_eligible:false,
   artifacts:{'budget-go.wasm':entry(wasm),'wasm_exec.cjs':entry(runtime)}}));
  const reads=[];
  const processor=await loadProcessor(directory,{
   async read(path,...args){
    reads.push(basename(path));const value=await readFile(path,...args);
    if(basename(path)==='wasm_exec.cjs') {
     await writeFile(runtimePath,'throw new Error("runtime reread");');
     await writeFile(join(directory,'budget-go.wasm'),'different wasm bytes');
    }
    return value;
   },
   async instantiate(buffer){assert.deepEqual(buffer,wasm);return {instance:{exports:{mem:{buffer:new ArrayBuffer(65536)}}}};},
  });
  assert.equal(globalThis.phase7RuntimeSource,'verified');
  assert.deepEqual(reads,['build.json','budget-go.wasm','wasm_exec.cjs']);
  assert.equal(processor.processLine('{}'),'{"kind":"executed"}');
 } finally {
  delete require.cache[runtimePath];
  delete globalThis.Go;delete globalThis.phase7BudgetGo;delete globalThis.phase7RuntimeSource;
  await rm(directory,{recursive:true,force:true});
 }
});
