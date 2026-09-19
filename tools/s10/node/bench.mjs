import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { loadavg, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { socketServer } from "./socket.mjs";

const [samplesText = "7", iterationsText = "100"] = process.argv.slice(2);
const samplesCount = Number(samplesText), iterations = Number(iterationsText);
assert(Number.isSafeInteger(samplesCount) && samplesCount > 0);
assert(Number.isSafeInteger(iterations) && iterations > 0);
const { Parser } = createRequire(import.meta.url)(resolve("target/s10/node/ts_node.node"));
const adapter = new Parser();
const template = readFileSync("tools/s10/parser/ten-kib.ts", "utf8");
assert.equal(Buffer.byteLength(template), 10240);
// Use a lower-case source name on case-insensitive native hosts so both
// default parser options and the server's canonical path name the same file.
const cwd = mkdtempSync(join(tmpdir(), "ts-rust-s10-node-"));
const physicalFilename = join(cwd, "input.ts");
writeFileSync(physicalFilename, template);
const api = await socketServer(resolve("target/s10/go/tsgo"), cwd,
  join(tmpdir(), `ts-rust-s10-${process.pid}.sock`));
let serial = 0;
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
try {
  const initialized = await api.call("initialize");
  const filename = initialized.useCaseSensitiveFileNames ? physicalFilename : physicalFilename.toLowerCase();
  const name = Buffer.from(filename);
  const base = await api.call("createProgram", {
    rootFiles: [filename], createProgramOptions: { compilerOptions: { noLib: true, types: [] } },
  });
  function inputs() {
    return Array.from({ length: iterations }, () => {
      const text = template.slice(0, -9) + String(++serial).padStart(8, "0") + "\n";
      assert.equal(Buffer.byteLength(text), 10240);
      return { text, source: Buffer.from(text) };
    });
  }
  async function batch(runtime, rows) {
    const encoded = [];
    const start = process.hrtime.bigint();
    for (const row of rows) {
      if (runtime === "rust") {
        encoded.push(adapter.parseAndEncode(row.source, name, 3));
      } else {
        const changed = await api.call("updateTemporarySnapshot", {
          snapshot: base.snapshot, file: filename, newText: row.text,
        });
        assert.notEqual(changed.snapshot, base.snapshot, "fresh source snapshot");
        const data = await api.call("getSourceFile", {
          snapshot: changed.snapshot, project: base.project.id, file: filename,
        });
        encoded.push(Buffer.from(data.data, "base64"));
        await api.call("release", { snapshot: changed.snapshot });
      }
    }
    return { elapsed_ns: Number(process.hrtime.bigint() - start), encoded };
  }
  const batches = [];
  for (let index = -1; index < samplesCount; index++) {
    const rows = inputs();
    const order = index % 2 === 0 ? ["go", "rust"] : ["rust", "go"];
    const load_before = loadavg();
    const results = {};
    for (const runtime of order) results[runtime] = await batch(runtime, rows);
    for (let i = 0; i < rows.length; i++) {
      assert.deepEqual(results.rust.encoded[i], results.go.encoded[i], `sample ${index}, input ${i}`);
    }
    batches.push({ index, order, load_before, load_after: loadavg(),
      elapsed_ns: { rust: results.rust.elapsed_ns, go: results.go.elapsed_ns },
      outputs: results.rust.encoded.map(sha), sources: rows.map(row => sha(row.source)),
    });
  }
  await api.call("release", { snapshot: base.snapshot });
  console.log(JSON.stringify({ version: 1, node: process.version, filename, iterations,
    bytes_per_file: 10240, fresh_source_each_iteration: true,
    in_process: "Node-API Parser object; persistent reserved-stack worker; synchronous owned requests",
    socket_sequence: ["updateTemporarySnapshot", "getSourceFile", "release"],
    socket_extra_work: "snapshot update, program loading/binding, reference release, JSON-RPC/base64 transport",
    warmup: batches[0], samples: batches.slice(1), server_stderr: api.stderr,
  }));
} finally { adapter.close(); await api.close(); }
