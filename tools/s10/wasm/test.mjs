import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { createInstance } from "./instance.mjs";
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
  session.dispose();
  assert.throws(() => session.diagnostics(), /disposed/);
  checker.dispose();
}

// Test the controller's failure contract independently of engine stack limits.
// An actual corpus trap is recorded by the capture runner, never swallowed.
let destroyed = false;
let detached = false;
const failed = createInstance(() => ({
  initSync() {},
  parse() { throw new WebAssembly.RuntimeError("injected trap"); },
  MemoryHost: class {
    free() { destroyed = true; }
    __destroy_into_raw() { detached = true; return 1; }
  },
}), module);
const held = failed.createHost(bytes("/"));
assert.throws(() => failed.parse(bytes(""), bytes("/a.ts"), 3), /injected trap/);
assert.equal(failed.state, "failed");
assert.throws(() => failed.parse(bytes(""), bytes("/a.ts"), 3), /failed/);
held.dispose();
assert.equal(detached, true, "automatic finalizer must be detached after trap");
assert.equal(destroyed, false, "a trapped instance must not be reentered for destructors");
console.log(JSON.stringify({ mode, fixtures: fixtures.length, imports, ownership: "passed" }));
