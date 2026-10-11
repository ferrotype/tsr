import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { createCompilerProcessor, lineReader, lineWriter, consumeCompiler, loadCompilerProcessor } from "./compiler-wasm.mjs";

function factory(execute, free = () => {}) {
  return () => ({ initSync() {}, Compiler: class { execute = execute; free = free; } });
}
const response = '{"kind":"response","value":{"version":1,"id":9007199254740993,"reply":{"reply":"released"}}}';

test("nested callbacks preserve raw command/reply and outer response bytes", () => {
  const command = '{"version":1,"command":{"command":"release","id":9007199254740993}}';
  const reply = '{"reply":"continue"}';
  const rows = [];
  const processor = createCompilerProcessor(factory(line => {
    assert.equal(line, command);
    assert.equal(globalThis.tsrPortableCompilerCallback('{"callback":"batch"}'), reply);
    return response;
  }), {}, call => { rows.push(call); return reply; });
  assert.equal(processor.processLine(command), response);
  assert.deepEqual(rows, ['{"callback":"batch"}']);
  assert.equal(Object.hasOwn(globalThis, "tsrPortableCompilerCallback"), false);
  processor.dispose();
});

test("trap is terminal, no destructor/retry enters poisoned wasm, fresh instance works", () => {
  let calls = 0;
  let frees = 0;
  const processor = createCompilerProcessor(factory(() => { calls++; throw new WebAssembly.RuntimeError("unreachable"); }, () => frees++), {}, () => "");
  assert.throws(() => processor.processLine("{}"), /unreachable/);
  assert.equal(processor.state, "failed");
  assert.throws(() => processor.processLine("{}"), /instance is failed/);
  processor.dispose();
  assert.equal(calls, 1);
  assert.equal(frees, 0);
  const fresh = createCompilerProcessor(factory(() => response), {}, () => "");
  assert.equal(fresh.processLine("{}"), response);
  fresh.dispose();
});

test("framed command failure is written then fails child loop without hiding row", () => {
  const rows = [];
  const input = ["{}", "not retried"];
  const processor = createCompilerProcessor(factory(() => '{"kind":"failure","value":"bad request"}'), {}, () => "");
  assert.throws(() => consumeCompiler(processor, () => input.shift() ?? null, row => rows.push(row)), /command failed/);
  assert.deepEqual(rows, ['{"kind":"failure","value":"bad request"}']);
  assert.deepEqual(input, ["not retried"]);
});

test("synchronous reader handles chunk boundaries, callback replies and malformed UTF8", () => {
  const chunks = [Buffer.from('{"a":'), Buffer.from('1}\n{"reply":"continue"}\nlast')];
  const receive = lineReader(buffer => { const chunk = chunks.shift(); if (!chunk) return 0; chunk.copy(buffer); return chunk.length; });
  assert.deepEqual([receive(), receive(), receive(), receive()], ['{"a":1}', '{"reply":"continue"}', "last", null]);
  let first = true;
  const malformed = lineReader(buffer => { if (!first) return 0; first = false; buffer[0] = 255; buffer[1] = 10; return 2; });
  assert.throws(malformed, /encoded data/);
});

test("missing artifact fails explicitly; --compiler shim never invokes native fallback", async () => {
  await assert.rejects(loadCompilerProcessor(undefined, () => ""), /TSR_WASM_COMPILER_ARTIFACT_DIR/);
  const child = spawnSync("tools/phase7/portable/wasm-consumer", ["--compiler"], {
    input: "{}\n", encoding: "utf8", env: { ...process.env, TSR_WASM_COMPILER_ARTIFACT_DIR: "" },
  });
  assert.equal(child.status, 1);
  assert.equal(child.stdout, "");
  assert.match(child.stderr, /TSR_WASM_COMPILER_ARTIFACT_DIR/);
});

test("partial writes retain the complete UTF8 frame and zero progress fails", () => {
  const chunks = [];
  const send = lineWriter(buffer => { const chunk = buffer.subarray(0, 2); chunks.push(chunk); return chunk.length; });
  send('{"text":"😀"}');
  assert.equal(Buffer.concat(chunks).toString("utf8"), '{"text":"😀"}\n');
  assert.throws(() => lineWriter(() => 0)("{}"), /invalid progress/);
});
