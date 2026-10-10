// One pinned benchmark file of packages/typescript, unchanged, and its
// tinybench tasks' results as JSON (docs/PHASE7-plan.md, R0 item 4):
//
//   node --conditions @typescript/source --experimental-import-meta-resolve \
//       capture.mjs BENCH_FILE OUT_FILE [single]
//
// The bench module builds its Bench privately and calls `table()` once after
// running it; patching that prototype method reads the tasks without touching
// the task bodies. `tinybench` is resolved from the bench file, so both
// modules share one instance. `single` passes the file's own
// singleIteration option (the memory pass).
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const [benchFile, outFile, single] = process.argv.slice(2);
const benchUrl = pathToFileURL(benchFile).href;
const { Bench } = await import(import.meta.resolve("tinybench", benchUrl));

const benches = [];
const table = Bench.prototype.table;
Bench.prototype.table = function (...args) {
    benches.push({
        name: this.name,
        tasks: this.tasks.map(task => ({ name: task.name, result: plain(task.result) })),
    });
    return table.apply(this, args);
};

const bench = await import(benchUrl);
await bench.runBenchmarks(single ? { singleIteration: true } : {});
writeFileSync(outFile, JSON.stringify({ node: process.version, benches }));

function plain(value) {
    return JSON.parse(JSON.stringify(value ?? null, (_key, item) =>
        item instanceof Error ? { error: String(item) } : ArrayBuffer.isView(item) ? Array.from(item) : item));
}
