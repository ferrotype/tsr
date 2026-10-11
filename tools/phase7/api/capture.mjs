// One pinned benchmark file of packages/typescript, unchanged, and its
// tinybench tasks' results as JSON (docs/PHASE7-plan.md, R0 item 4):
//
//   node --conditions @typescript/source --experimental-import-meta-resolve \
//       capture.mjs BENCH_FILE OUT_FILE [single]
//
// Capture the privately constructed Bench before execution. Run both clients
// with Bench.run(), whose synchronous task timer is identical to runSync()'s,
// so untimed teardown can await process exit. Task bodies/options stay pinned;
// spawn API additionally uses the pin's teardown after EACH iteration.
// `single` passes the file's own singleIteration option (the memory pass).
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { trackChildren } from "./children.mjs";

const [benchFile, outFile, single] = process.argv.slice(2);
const benchUrl = pathToFileURL(benchFile).href;
const { Bench } = await import(import.meta.resolve("tinybench", benchUrl));

const children = trackChildren();
const stop = new Error("captured Bench before running it");
let captured;
const run = Bench.prototype.run;
const runSync = Bench.prototype.runSync;
const add = Bench.prototype.add;
Bench.prototype.run = Bench.prototype.runSync = function () { captured = this; throw stop; };
Bench.prototype.add = function (name, fn, options) {
    if (name === "spawn API") {
        const bench = this;
        const afterEach = options?.afterEach;
        options = { ...options, async afterEach(mode) {
            try { await afterEach?.call(this, mode); }
            finally { await bench.teardown(this, mode); }
        } };
    }
    return add.call(this, name, fn, options);
};

try {
    const module = await import(benchUrl);
    try { await module.runBenchmarks(single ? { singleIteration: true } : {}); }
    catch (error) { if (error !== stop) throw error; }
    if (!captured) throw new Error("pinned file did not construct a Bench");
    // The benchmark's identifier census creates and closes a server first.
    await children.drain();
    const teardown = captured.teardown;
    captured.teardown = async function (...args) {
        try { await teardown.apply(this, args); }
        finally { await children.drain(); }
    };
    await run.call(captured);
    const lifecycle = children.report();
    if (lifecycle.remaining || lifecycle.spawned !== lifecycle.reaped || lifecycle.peak > 1) {
        throw new Error(`API servers overlapped or leaked: ${JSON.stringify(lifecycle)}`);
    }
    writeFileSync(outFile, JSON.stringify({ node: process.version, lifecycle, benches: [{
        name: captured.name,
        tasks: captured.tasks.map(task => ({ name: task.name, result: plain(task.result) })),
    }] }));
} finally {
    Bench.prototype.run = run;
    Bench.prototype.runSync = runSync;
    Bench.prototype.add = add;
    await children.dispose();
}

function plain(value) {
    return JSON.parse(JSON.stringify(value ?? null, (_key, item) =>
        item instanceof Error ? { error: String(item) } : ArrayBuffer.isView(item) ? Array.from(item) : item));
}
