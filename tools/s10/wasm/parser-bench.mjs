import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { loadavg } from "node:os";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createInstance } from "./instance.mjs";
import { goParser } from "./go.mjs";

const [manifestPath, parserPath = "target/s10/parser", sampleText = "7"] = process.argv.slice(2);
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const directory = resolve(parserPath);
const { createBindings } = await import(pathToFileURL(`${directory}/bindings.mjs`));
const moduleBytes = readFileSync(`${directory}/parser_bg.wasm`);
const rust = createInstance(createBindings, new WebAssembly.Module(moduleBytes));
const go = await goParser();
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const inputs = manifest.files.map(row => {
  const source = readFileSync(row.local);
  assert.equal(sha(source), row.sha256, row.filename);
  // The frozen VS Code inventory contains no BOM files: assert this instead
  // of silently timing different physical-file decoding on the two sides.
  assert(!["fffe", "feff"].includes(source.subarray(0, 2).toString("hex")));
  assert.notEqual(source.subarray(0, 3).toString("hex"), "efbbbf");
  return { ...row, source, args: [source, Buffer.from(row.filename), row.script_kind, row.jsx, row.force] };
});
const expected = [0, 0, 0];
const observations = [];
for (const [index, row] of inputs.entries()) {
  const counts = Array.from(rust.parse(...row.args));
  assert.deepEqual(counts, go.parse(...row.args), row.filename);
  const left = rust.parseAndEncode(...row.args);
  const right = go.parse_and_encode(...row.args);
  assert.equal(sha(left), sha(right), row.filename + " protocol bytes");
  counts.forEach((n, i) => { expected[i] += n; });
  observations.push({ filename: row.filename, counts, encoded_sha256: sha(left) });
  if (index % 1000 === 0) process.stderr.write(`parser parity ${index + 1}/${inputs.length}\n`);
}
go.collect();
function batch(runtime) {
  const counts = [0, 0, 0];
  const start = process.hrtime.bigint();
  for (const row of inputs) {
    const result = runtime === "rust" ? rust.parse(...row.args) : go.parse(...row.args);
    for (let i = 0; i < counts.length; i++) counts[i] += result[i];
  }
  if (runtime === "go") go.collect();
  const elapsed = Number(process.hrtime.bigint() - start);
  assert.deepEqual(counts, expected, runtime + " timed operation inventory");
  return elapsed;
}
const warmup = { rust: batch("rust"), go: batch("go") };
const samples = [];
for (let i = 0; i < Number(sampleText); i++) {
  const order = i % 2 === 0 ? ["rust", "go"] : ["go", "rust"];
  const sample = { index: i, order, load_before: loadavg(), elapsed_ns: {} };
  for (const runtime of order) sample.elapsed_ns[runtime] = batch(runtime);
  sample.load_after = loadavg();
  samples.push(sample);
  process.stderr.write(`parser sample ${i + 1}/${sampleText}\n`);
}
rust.dispose();
console.log(JSON.stringify({
  version: 1, node: process.version, files: inputs.length,
  source_bytes: inputs.reduce((sum, row) => sum + row.source.length, 0),
  gc_policy: "Rust drops each AST; Go final runtime.GC included in every batch",
  observations, warmup, samples,
}));
