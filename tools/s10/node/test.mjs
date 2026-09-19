import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
const { Parser, parseAndEncode } = createRequire(import.meta.url)(resolve("target/s10/node/tsr_node.node"));
const fixtures = JSON.parse(readFileSync("tools/s10/parser/fixtures.json", "utf8"));
const options = [
  { id: "jsx-module", source: [...Buffer.from("const x = <div/>;")], name: [...Buffer.from("/a.tsx")], kind: 4, jsx: true },
  { id: "forced-module", source: [...Buffer.from("const x = 1;")], name: [...Buffer.from("/a.ts")], kind: 3, force: true },
];
const requests = [...fixtures, ...options];
const native = spawnSync("target/release/examples/parser", [], {
  input: requests.map(row => JSON.stringify(row)).join("\n") + "\n",
  encoding: "utf8", maxBuffer: 32 * 1024 * 1024, timeout: 60000,
});
assert.equal(native.status, 0, native.stderr);
const expected = native.stdout.trim().split("\n").map(JSON.parse);
const held = [];
for (let repetition = 0; repetition < 4; repetition++) {
  const worker = new Parser();
  for (const [i, row] of fixtures.entries()) {
    const args = [Buffer.from(row.source), Buffer.from(row.name), row.kind];
    const output = worker.parseAndEncode(...args);
    assert.deepEqual([...output], expected[i].bytes);
    assert.deepEqual(parseAndEncode(...args), output);
    held.push([output, expected[i].bytes]);
  }
  worker.close();
  worker.close();
  assert.throws(() => worker.parseAndEncode(Buffer.from(""), Buffer.from("a.ts"), 3), /closed/);
}
for (const [output, bytes] of held) assert.deepEqual([...output], bytes);
const worker = new Parser();
try {
  for (const [i, row] of options.entries()) {
    const args = [Buffer.from(row.source), Buffer.from(row.name), row.kind, row.jsx ?? false, row.force ?? false];
    assert.deepEqual([...worker.parseAndEncode(...args)], expected[fixtures.length + i].bytes);
    assert.deepEqual(parseAndEncode(...args), worker.parseAndEncode(...args));
    assert.notDeepEqual(worker.parseAndEncode(...args.slice(0, 3)), worker.parseAndEncode(...args));
  }
} finally { worker.close(); }
console.log(JSON.stringify({ fixtures: fixtures.length, lifecycles: 4, retained_outputs: held.length }));
