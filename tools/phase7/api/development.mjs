// Bounded batched-symbol development workload using the pinned public client.
// Arguments: SERVER MODE TASK ITERATIONS OUTPUT
// The parent releases READY with "run" and DONE with "collect".
// Hashing, artifact IO and teardown wait until both peers have stopped timing.
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { trackChildren } from "./children.mjs";
import { symbolObserver } from "./symbols.mjs";

const [server, mode, task, count, output] = process.argv.slice(2);
const iterations = Number(count);
if (!["sync", "async"].includes(mode) || !["location", "position"].includes(task)
    || !Number.isSafeInteger(iterations) || iterations < 1 || process.argv.length !== 7) {
    throw new Error("expected SERVER sync|async location|position POSITIVE_ITERATIONS OUTPUT");
}
const root = fileURLToPath(new URL("../../../upstream/", import.meta.url));
const bench = new URL(`packages/typescript/test/${mode}/api.bench.ts`, new URL("../../../upstream/", import.meta.url)).href;
const children = trackChildren();
const { API } = await import(import.meta.resolve(`@typescript/typescript/unstable/${mode}`, bench));
const { SyntaxKind } = await import(import.meta.resolve("@typescript/typescript/unstable/ast", bench));
const lines = createInterface({ input: process.stdin });
const commands = lines[Symbol.asyncIterator]();
async function phase(message, expected) {
    process.stdout.write(`${message}\n`);
    const command = await commands.next();
    if (command.done || command.value !== expected) throw new Error(`expected ${expected} after ${message}`);
}
try {
    const api = new API({ cwd: root, tsserverPath: server });
    const snapshot = await api.updateSnapshot({ openProject: "tsc/testdata/fixtures/compiler/tsconfig.json" });
    const project = snapshot.getProjects()[0];
    if (!project) throw new Error("benchmark project is absent");
    await project.checker.getSymbolAtPosition("core.ts", 0);
    const file = await project.program.getSourceFile("program.ts");
    if (!file) throw new Error("program.ts is absent");
    function identifiers() {
        const nodes = [];
        file.forEachChild(function visit(node) {
            if (node.kind === SyntaxKind.Identifier) nodes.push(node);
            node.forEachChild(visit);
        });
        return nodes;
    }
    const inputs = identifiers();
    if (!inputs.length) throw new Error("identifier traversal is empty");
    const samples = [], answers = [];
    await phase("READY", "run");
    for (let i = 0; i < iterations; i++) {
        const start = process.hrtime.bigint();
        const nodes = identifiers(); // The pinned task includes traversal/argument construction.
        const pending = task === "location" ? project.checker.getSymbolAtLocation(nodes)
            : project.checker.getSymbolAtPosition("program.ts", nodes.map(node => node.pos));
        const result = mode === "async" ? await pending : pending;
        samples.push(Number(process.hrtime.bigint() - start));
        answers.push(result);
    }
    await phase("DONE", "collect");
    if (answers.some(values => values.length !== inputs.length)) throw new Error("query dropped results");
    const symbol = symbolObserver(root);
    const work = { roots: project.rootFiles.map(path => path.replaceAll(root, "@UPSTREAM@/")),
        inputs: inputs.map(node => [node.pos, node.end]), answers: answers.map(values => values.map(symbol)) };
    const observed = { count: inputs.length, iterations,
        sha256: createHash("sha256").update(JSON.stringify(work)).digest("hex") };
    await api.close();
    await children.drain();
    const lifecycle = children.report();
    if (lifecycle.remaining || lifecycle.spawned !== 1 || lifecycle.reaped !== 1 || lifecycle.peak !== 1) {
        throw new Error(`server lifecycle differs: ${JSON.stringify(lifecycle)}`);
    }
    writeFileSync(output, JSON.stringify({ method: "development concurrent",
        node: process.version, mode, task, iterations, samples_ns: samples, work: observed, lifecycle }));
} finally {
    lines?.close();
    await children.dispose();
}
