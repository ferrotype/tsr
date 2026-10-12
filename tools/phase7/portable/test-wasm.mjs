import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createProcessor, loadProcessor } from "./wasm.mjs";

const encode = value => new TextEncoder().encode(typeof value === "string" ? value : JSON.stringify(value));
const response = () => ({ version: 1, output: [101], source_map: [], diagnostics: [], sources: [] });
const request = () => ({
  version: 1, declaration: false, source: [255, 237, 160, 128], filename: [254, 46, 116, 115],
  compiler_options: Array.from(encode('{"paths":{"10":["a"],"2":["b"]}}')), report_diagnostics: true,
});
const factory = operation => () => ({ initSync() {}, transpile_module: operation, transpile_declaration: operation });

test("byte inputs and raw ordered compiler options reach the production export", () => {
  const calls = [];
  const processor = createProcessor(factory((...args) => {
    calls.push(args);
    return encode(response());
  }), {});
  const value = request();
  assert.deepEqual(JSON.parse(processor.processLine(JSON.stringify(value))), response());
  assert.deepEqual(calls[0], [Uint8Array.from(value.source), Uint8Array.from(value.filename),
    '{"paths":{"10":["a"],"2":["b"]}}', true]);
  value.declaration = true;
  processor.processLine(JSON.stringify(value));
  assert.equal(calls.length, 2);
  processor.dispose();
  assert.equal(processor.state, "disposed");
});

test("malformed requests fail without entering wasm", () => {
  let calls = 0;
  const processor = createProcessor(factory(() => { calls++; return encode(response()); }), {});
  for (const change of [
    value => { value.version = 2; },
    value => { value.extra = true; },
    value => { delete value.report_diagnostics; },
    value => { value.declaration = 1; },
    value => { value.source = [256]; },
    value => { value.compiler_options = [255]; },
  ]) {
    const value = request();
    change(value);
    assert.throws(() => processor.processLine(JSON.stringify(value)));
  }
  const json = JSON.stringify(request());
  assert.throws(() => processor.processLine(json.replace('"version":1', '"version":1,"\\u0076ersion":1')), /duplicate field/);
  assert.throws(() => processor.processLine(""));
  assert.throws(() => processor.processLine("{}"));
  assert.equal(calls, 0);
  processor.dispose();
});

test("the raw options JSON keeps a BOM for Rust to reject", () => {
  let observed;
  const processor = createProcessor(factory((_source, _filename, options) => {
    observed = options;
    throw "ts-wasm-api-error:compiler options JSON: BOM";
  }), {});
  const value = request();
  value.compiler_options = [239, 187, 191, 123, 125];
  assert.throws(() => processor.processLine(JSON.stringify(value)), /compiler options JSON: BOM/);
  assert.equal(observed, "\ufeff{}");
  assert.equal(processor.state, "live");
  processor.dispose();
});

test("a trap poisons the instance and the failed row is not retried", () => {
  let instances = 0;
  let calls = 0;
  const bindings = () => {
    const instance = ++instances;
    return factory(() => {
      calls++;
      if (instance === 1) throw new WebAssembly.RuntimeError("unreachable");
      return encode(response());
    })();
  };
  const processor = createProcessor(bindings, {});
  const line = JSON.stringify(request());
  assert.throws(() => processor.processLine(line), /unreachable/);
  assert.equal(processor.state, "failed");
  assert.equal(instances, 1);
  assert.throws(() => processor.processLine(line), /instance is failed/);
  assert.equal(calls, 1);
  processor.dispose();
  const fresh = createProcessor(bindings, {});
  assert.deepEqual(JSON.parse(fresh.processLine(line)), response());
  assert.equal(instances, 2);
  fresh.dispose();
});

test("malformed production responses fail validation", () => {
  for (const changed of [
    { ...response(), version: 2 },
    { ...response(), extra: true },
    { ...response(), output: [256] },
    { ...response(), sources: [{ name: [], path: [], text: [], canonical_name: [] }] },
    { ...response(), diagnostics: [{}] },
  ]) {
    const processor = createProcessor(factory(() => encode(changed)), {});
    assert.throws(() => processor.processLine(JSON.stringify(request())));
    processor.dispose();
  }
  const duplicate = JSON.stringify(response()).replace('"version":1', '"version":1,"version":1');
  const processor = createProcessor(factory(() => encode(duplicate)), {});
  assert.throws(() => processor.processLine(JSON.stringify(request())), /duplicate field/);
  processor.dispose();
});

test("empty, malformed and trapped child responses fail with no stdout row", () => {
  const module = new URL("./wasm.mjs", import.meta.url).href;
  for (const operation of [
    "return new Uint8Array();",
    "return new TextEncoder().encode('{');",
    "return new Uint8Array([255]);",
    "return undefined;",
    "throw new WebAssembly.RuntimeError('unreachable');",
    "throw 'ts-wasm-api-error:bad options';",
  ]) {
    const script = `import { createProcessor, consume } from ${JSON.stringify(module)};
      const operation = () => { ${operation} };
      const bindings = () => ({ initSync() {}, transpile_module: operation, transpile_declaration: operation });
      await consume(process.stdin, process.stdout, createProcessor(bindings, {}));`;
    const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
      input: JSON.stringify(request()) + "\n", encoding: "utf8",
    });
    assert.equal(child.error, undefined);
    assert.notEqual(child.status, 0, operation);
    assert.equal(child.stdout, "", operation);
    assert.notEqual(child.stderr, "", operation);
  }
});

test("missing or stale artifact inputs cannot select another backend", async () => {
  await assert.rejects(loadProcessor(undefined), /TSR_WASM_ARTIFACT_DIR/);
  const directory = mkdtempSync(resolve(tmpdir(), "phase7-wasm-"));
  try {
    const wasm = Buffer.from("this is not wasm");
    const bindings = Buffer.from("export function createBindings() {}");
    const artifacts = {};
    for (const [name, data] of [["checker_bg.wasm", wasm], ["bindings.mjs", bindings]]) {
      writeFileSync(resolve(directory, name), data);
      artifacts[name] = { sha256: createHash("sha256").update(data).digest("hex") };
    }
    const record = { mode: "checker", command: ["cargo", "build", "-p", "tsr_wasm", "--features", "checker"], artifacts };
    writeFileSync(resolve(directory, "build.json"), JSON.stringify(record));
    await assert.rejects(loadProcessor(directory), /WebAssembly.Module/);
    writeFileSync(resolve(directory, "checker_bg.wasm"), Buffer.from("different bytes"));
    await assert.rejects(loadProcessor(directory), /artifact bytes differ/);
    record.mode = "corpus";
    writeFileSync(resolve(directory, "build.json"), JSON.stringify(record));
    await assert.rejects(loadProcessor(directory), /not the production/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("the executable shim requires explicit artifacts and pins the engine stack", () => {
  const shim = fileURLToPath(new URL("./wasm-consumer", import.meta.url));
  const pin = JSON.parse(readFileSync(new URL("../../s10/toolchains.json", import.meta.url)));
  const source = readFileSync(shim, "utf8");
  for (const flag of pin.node_flags) assert.ok(source.includes(flag));
  const env = { ...process.env };
  delete env.TSR_WASM_ARTIFACT_DIR;
  const child = spawnSync(shim, [], { input: JSON.stringify(request()) + "\n", encoding: "utf8", env });
  assert.equal(child.error, undefined);
  assert.notEqual(child.status, 0);
  assert.equal(child.stdout, "");
  assert.match(child.stderr, /TSR_WASM_ARTIFACT_DIR/);
});

test("transpile identity verifies the production ABI and binds actual engine and artifact bytes", async () => {
  const directory = mkdtempSync(resolve(tmpdir(), "phase7-transpile-identity-"));
  const leb = number => { const bytes = []; do { const byte = number & 127; number >>>= 7; bytes.push(byte | (number ? 128 : 0)); } while (number); return bytes; };
  const string = value => { const bytes = Buffer.from(value); return [...leb(bytes.length), ...bytes]; };
  const section = (id, bytes) => [id, ...leb(bytes.length), ...bytes];
  const names = ["__wbindgen_init_externref_table", "__wbindgen_cast_0000000000000001", "__wbg___wbindgen_throw_344f42d3211c4765"];
  const wasm = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0, ...section(1, [1, 96, 0, 0]),
    ...section(2, [names.length, ...names.flatMap(name => [...string("./checker_bg.js"), ...string(name), 0, 0])]),
    ...section(7, [2, ...string("transpile_module"), 0, 0, ...string("transpile_declaration"), 0, 0]),
  ]);
  const bindings = Buffer.from('throw new Error("identity probe must not execute generated JS");');
  const digest = bytes => createHash("sha256").update(bytes).digest("hex");
  try {
    const files = { "checker_bg.wasm": wasm, "bindings.mjs": bindings };
    for (const [name, bytes] of Object.entries(files)) writeFileSync(resolve(directory, name), bytes);
    const record = { mode: "checker", command: ["cargo", "build", "-p", "tsr_wasm", "--features", "checker"],
      artifacts: Object.fromEntries(Object.entries(files).map(([name, bytes]) => [name, { sha256: digest(bytes), bytes: bytes.length }])) };
    const recordBytes = JSON.stringify(record);
    writeFileSync(resolve(directory, "build.json"), recordBytes);
    const identity = await loadProcessor(directory, true);
    assert.equal(identity.runtime, "wasm");
    assert.equal(identity.family, "transpile");
    assert.equal(identity.build_sha256, digest(recordBytes));
    assert.equal(identity.artifacts["checker_bg.wasm"], digest(wasm));
    const child = spawnSync(fileURLToPath(new URL("./wasm-consumer", import.meta.url)), ["--identity"], {
      encoding: "utf8", env: { ...process.env, TSR_WASM_ARTIFACT_DIR: directory },
    });
    assert.equal(child.status, 0, child.stderr);
    assert.deepEqual(JSON.parse(child.stdout).engine.flags, ["--stack-size=4096"]);
    writeFileSync(resolve(directory, "bindings.mjs"), "changed after build");
    await assert.rejects(loadProcessor(directory, true), /artifact bytes differ/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
