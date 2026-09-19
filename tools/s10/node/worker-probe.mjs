// Development attribution only. Build ts_node with --features worker-probe,
// copy its library to a separate .node path, then pass that path here.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { loadavg } from "node:os";
import { resolve } from "node:path";

const [binary, samplesText = "21", iterationsText = "100"] = process.argv.slice(2);
assert(binary, "supply the instrumented .node binary");
const samples = Number(samplesText), iterations = Number(iterationsText);
assert(Number.isSafeInteger(samples) && samples > 0);
assert(Number.isSafeInteger(iterations) && iterations > 0);
const { Parser } = createRequire(import.meta.url)(resolve(binary));
const parser = new Parser();
const template = readFileSync("tools/s10/parser/ten-kib.ts", "utf8");
const name = Buffer.from("/s10/probe.ts");
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
assert.equal(Buffer.byteLength(template), 10240);
let serial = 0;
const batches = [];
try {
  for (let index = -1; index < samples; index++) {
    const inputs = Array.from({ length: iterations }, () => {
      const text = template.slice(0, -9) + String(++serial).padStart(8, "0") + "\n";
      assert.equal(Buffer.byteLength(text), 10240);
      return Buffer.from(text);
    });
    const order = index % 2 === 0 ? ["normal", "measured"] : ["measured", "normal"];
    const load_before = loadavg();
    const results = {};
    for (const mode of order) {
      const outputs = [];
      const start = process.hrtime.bigint();
      for (const input of inputs) outputs.push(mode === "normal"
        ? parser.parseAndEncode(input, name, 3)
        : parser.measureParseAndEncode(input, name, 3));
      results[mode] = { elapsed_ns: Number(process.hrtime.bigint() - start), outputs };
    }
    const totals = { prepareNs: 0, dispatchNs: 0, workNs: 0, returnNs: 0 };
    for (let i = 0; i < inputs.length; i++) {
      const measured = results.measured.outputs[i];
      assert.deepEqual(measured.output, results.normal.outputs[i], `sample ${index}, call ${i}`);
      for (const key of Object.keys(totals)) {
        assert(Number.isFinite(measured[key]) && measured[key] >= 0, key);
        totals[key] += measured[key];
      }
    }
    batches.push({ index, order, load_before, load_after: loadavg(), totals,
      elapsed_ns: { normal: results.normal.elapsed_ns, measured: results.measured.elapsed_ns },
      sources: inputs.map(sha), outputs: results.normal.outputs.map(sha) });
  }
  console.log(JSON.stringify({ version: 1, purpose: "Node worker attribution, not E8 acceptance",
    node: process.version, binary: resolve(binary), binary_sha256: sha(readFileSync(binary)),
    iterations, bytes_per_file: 10240, fresh_source_each_iteration: true,
    boundaries: { prepareNs: "input copying and sender check",
      dispatchNs: "reply channel creation, send, worker wakeup",
      workNs: "production parse, encode and request disposal on reserved stack",
      returnNs: "response send and caller wakeup" },
    excluded: "N-API entry/exit, output conversion and JS bookkeeping are in wall time only",
    warmup: batches[0], samples: batches.slice(1) }));
} finally { parser.close(); }
