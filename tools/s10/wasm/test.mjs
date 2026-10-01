import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { createInstance, WasmApiError } from "./instance.mjs";
import { checkImports } from "./imports.mjs";

const directory = resolve(process.argv[2] ?? "target/s10/parser");
const mode = directory.split("/").at(-1);
const { createBindings } = await import(pathToFileURL(`${directory}/bindings.mjs`));
const module = new WebAssembly.Module(readFileSync(`${directory}/${mode}_bg.wasm`));
const imports = checkImports(module, mode);
const encoder = new TextEncoder();
const bytes = value => encoder.encode(value);
const fixtures = JSON.parse(readFileSync("tools/s10/parser/fixtures.json", "utf8"));
const native = spawnSync("target/release/examples/parser", [], {
  input: fixtures.map(row => JSON.stringify(row)).join("\n") + "\n",
  encoding: "utf8", maxBuffer: 32 * 1024 * 1024, timeout: 60000,
});
assert.equal(native.status, 0, native.stderr);
const expected = native.stdout.trim().split("\n").map(JSON.parse);
assert.equal(expected.length, fixtures.length);
const parser = createInstance(createBindings, module);
let prior;
for (const [index, fixture] of fixtures.entries()) {
  const source = Uint8Array.from(fixture.source);
  const name = Uint8Array.from(fixture.name);
  assert.deepEqual(Array.from(parser.parse(source, name, fixture.kind)), expected[index].counts, fixture.id);
  const encoded = parser.parseAndEncode(source, name, fixture.kind);
  assert.deepEqual(Array.from(encoded), expected[index].bytes, fixture.id);
  if (prior) assert.deepEqual(Array.from(prior.output), prior.expected, "output survives subsequent wasm calls");
  prior = { output: encoded, expected: expected[index].bytes };
}
const independent = createInstance(createBindings, module);
parser.dispose();
assert.throws(() => parser.parse(bytes("let x;"), bytes("/a.ts"), 3), /disposed/);
assert.deepEqual(Array.from(prior.output), prior.expected);
assert.equal(independent.parse(bytes("const x = 1;"), bytes("/a.ts"), 3)[2], 0);
assert.throws(() => independent.parse("not bytes", bytes("/a.ts"), 3), TypeError);
assert.equal(independent.state, "live", "JS argument validation doesn't poison Rust state");
independent.dispose();

if (mode !== "parser") {
  const checker = createInstance(createBindings, module);
  const host = checker.createHost(bytes("/"));
  host.addFile(bytes("/a.ts"), bytes("const value: string = 1;"), true);
  const session = host.compile({ strict: true, noLib: true });
  assert.throws(() => host.addDirectory(bytes("/later")), /disposed/);
  assert.equal(new TextDecoder().decode(session.typeAtPosition(bytes("/a.ts"), 6)), "string");
  assert(session.diagnostics().some(value => value.code === 2322));
  const siblingHost = checker.createHost(bytes("/"));
  siblingHost.addFile(bytes("/b.ts"), bytes("const other: number = 1;"), true);
  const sibling = siblingHost.compile({ strict: true, noLib: true });
  for (const query of [() => session.typeAtPosition(bytes("/missing.ts"), 0),
                       () => session.typeAtPosition(bytes("/a.ts"), 10000)]) {
    assert.throws(query, WasmApiError);
    assert.equal(checker.state, "live");
    assert.equal(new TextDecoder().decode(session.typeAtPosition(bytes("/a.ts"), 6)), "string");
    assert.equal(new TextDecoder().decode(sibling.typeAtPosition(bytes("/b.ts"), 6)), "number");
  }
  const badHost = checker.createHost(bytes("/"));
  assert.throws(() => badHost.compile({ target: "not-a-numeric-enum" }), WasmApiError);
  assert.throws(() => badHost.addDirectory(bytes("/consumed")), /disposed/);
  const serialHost = checker.createHost(bytes("/"));
  const cycle = {}; cycle.self = cycle;
  assert.throws(() => serialHost.compile(cycle), TypeError);
  serialHost.addDirectory(bytes("/still-live"));
  serialHost.dispose();
  const decode = value => new TextDecoder().decode(value);
  const emitHost = checker.createHost(bytes("/"));
  emitHost.addFile(bytes("/src/a.ts"), bytes("export const value: number = 1;\n"), true);
  const emitting = emitHost.compile({ declaration: true, module: 99, target: 9 });
  const emitted = emitting.emit();
  assert.equal(emitted.emit_skipped, false);
  assert.deepEqual(emitted.diagnostics, []);
  assert.deepEqual(emitted.files.map(file => decode(file.name)), ["/src/a.js", "/src/a.d.ts"]);
  assert.equal(decode(emitted.files[0].text), "export const value = 1;\n");
  assert.equal(decode(emitted.files[1].text), "export declare const value: number;\n");
  const declarations = emitting.emit({ files: [bytes("/src/a.ts")], emitOnly: 2 });
  assert.deepEqual(declarations.files.map(file => decode(file.name)), ["/src/a.d.ts"]);
  for (const request of [{ emitOnly: 3 }, { forceEmit: 1 }, { unknown: true }, { files: [bytes("/missing.ts")] }]) {
    assert.throws(() => emitting.emit(request), WasmApiError);
    assert.equal(checker.state, "live");
  }
  assert.throws(() => emitting.emit({ files: ["/src/a.ts"] }), TypeError);
  assert.equal(decode(emitting.emit().files[0].text), "export const value = 1;\n");
  emitting.retire();
  assert.throws(() => emitting.emit(), WasmApiError);
  assert.equal(checker.state, "live");
  emitting.dispose();
  session.retire();
  assert.throws(() => session.diagnostics(), WasmApiError);
  assert.equal(checker.state, "live");
  assert.equal(new TextDecoder().decode(sibling.typeAtPosition(bytes("/b.ts"), 6)), "number");
  sibling.dispose();
  session.dispose();
  assert.throws(() => session.diagnostics(), /disposed/);
  checker.dispose();
}

// Test the controller's failure contract independently of engine stack limits.
// An actual corpus trap is recorded by the capture runner, never swallowed.
for (const fault of [new WebAssembly.RuntimeError("injected trap"),
                     new RangeError("Maximum call stack size exceeded"), new Error("unexpected glue error")]) {
  let destroyed = false;
  let detached = false;
  const failed = createInstance(() => ({
    initSync() {},
    parse() { throw fault; },
    MemoryHost: class {
      free() { destroyed = true; }
      __destroy_into_raw() { detached = true; return 1; }
    },
  }), module);
  const held = failed.createHost(bytes("/"));
  assert.throws(() => failed.parse(bytes(""), bytes("/a.ts"), 3), error => error === fault);
  assert.equal(failed.state, "failed");
  assert.throws(() => failed.parse(bytes(""), bytes("/a.ts"), 3), /failed/);
  held.dispose();
  assert.equal(detached, true, "automatic finalizer must be detached after trap");
  assert.equal(destroyed, false, "a trapped instance must not be reentered for destructors");
}
console.log(JSON.stringify({ mode, fixtures: fixtures.length, imports, ownership: "passed" }));
