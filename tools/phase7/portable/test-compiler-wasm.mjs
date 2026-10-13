import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { createCompilerProcessor, lineReader, lineWriter, consumeCompiler, loadCompilerProcessor } from "./compiler-wasm.mjs";
import { checkCompilerImports } from "./compiler-imports.mjs";

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

const abi = [
  "__wbg_tsrPortableCompilerCallback_fedf4f42051b2f81",
  "__wbg___wbindgen_throw_344f42d3211c4765",
  "__wbg___wbindgen_debug_string_c25d447a39f5578f",
  "__wbindgen_init_externref_table",
];
const leb = number => { const bytes = []; do { const byte = number & 127; number >>>= 7; bytes.push(byte | (number ? 128 : 0)); } while (number); return bytes; };
const string = value => { const bytes = Buffer.from(value); return [...leb(bytes.length), ...bytes]; };
const section = (id, bytes) => [id, ...leb(bytes.length), ...bytes];
function wasmWithImports(names, module = "./compiler_bg.js") {
  return Buffer.from([0, 97, 115, 109, 1, 0, 0, 0,
    ...section(1, [1, 96, 0, 0]),
    ...section(2, [...leb(names.length), ...names.flatMap(name => [...string(module), ...string(name), 0, 0])]),
  ]);
}

test("compiler ABI policy rejects unknown and renamed host imports independently of build metadata", () => {
  assert.equal(checkCompilerImports(new WebAssembly.Module(wasmWithImports(abi))).length, 4);
  for (const [names, module] of [[abi.concat("host_filesystem"), "./compiler_bg.js"],
    [abi.slice(1), "./compiler_bg.js"], [abi, "host"], [[...abi.slice(0, 3), abi[0]], "./compiler_bg.js"]]) {
    assert.throws(() => checkCompilerImports(new WebAssembly.Module(wasmWithImports(names, module))), /unreviewed compiler wasm imports/);
  }
});

test("identity verifies wasm and ABI; a self-consistent record cannot authorize an unknown import", async () => {
  const directory = mkdtempSync(resolve(tmpdir(), "phase7-compiler-identity-"));
  const digest = data => createHash("sha256").update(data).digest("hex");
  try {
    for (const names of [abi, abi.concat("host_filesystem")]) {
      const wasm = wasmWithImports(names);
      const bindings = Buffer.from('throw new Error("identity probe must not execute generated JS");');
      const files = { "compiler_bg.wasm": wasm, "bindings.mjs": bindings };
      for (const [name, bytes] of Object.entries(files)) writeFileSync(resolve(directory, name), bytes);
      const record = { version: 1, mode: "phase7-test-compiler", package: "phase7_wasm_compiler", performance_eligible: false,
        imports: WebAssembly.Module.imports(new WebAssembly.Module(wasm)),
        artifacts: Object.fromEntries(Object.entries(files).map(([name, bytes]) => [name, { sha256: digest(bytes), bytes: bytes.length }])) };
      const recordBytes = JSON.stringify(record);
      writeFileSync(resolve(directory, "build.json"), recordBytes);
      if (names.length === abi.length) {
        const identity = await loadCompilerProcessor(directory, undefined, true);
        assert.equal(identity.runtime, "wasm");
        assert.equal(identity.family, "compiler");
        assert.equal(identity.build_sha256, digest(recordBytes));
        assert.equal(identity.artifacts["compiler_bg.wasm"], digest(wasm));
        const child = spawnSync("tools/phase7/portable/wasm-consumer", ["--compiler", "--identity"], {
          encoding: "utf8", env: { ...process.env, NODE_OPTIONS: "--max-old-space-size=4096", TSR_WASM_COMPILER_ARTIFACT_DIR: directory },
        });
        assert.equal(child.status, 0, child.stderr);
        assert.deepEqual(JSON.parse(child.stdout).engine.flags, ["--stack-size=4096"]);
        assert.equal(JSON.parse(child.stdout).engine.node_options, "--max-old-space-size=4096");
      } else {
        await assert.rejects(loadCompilerProcessor(directory, undefined, true), /unreviewed compiler wasm imports/);
        await assert.rejects(loadCompilerProcessor(directory, () => ""), /unreviewed compiler wasm imports/);
      }
    }
    writeFileSync(resolve(directory, "compiler_bg.wasm"), Buffer.from("changed after build"));
    await assert.rejects(loadCompilerProcessor(directory, undefined, true), /artifact bytes differ/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
