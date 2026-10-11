// Untimed answers for the server-dependent operations in the pinned API bench.
// Fresh clients per task, as in its beforeAll/teardown hooks. Compare public
// values and declaration handles, never process-local symbol/snapshot IDs.
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { trackChildren } from "./children.mjs";
import { symbolObserver } from "./symbols.mjs";

const [benchFile, outFile] = process.argv.slice(2);
const benchUrl = pathToFileURL(benchFile).href;
const mode = benchUrl.includes("/sync/") ? "sync" : "async";
const repoRoot = fileURLToPath(new URL("../../../../", benchUrl));
const { API } = await import(import.meta.resolve(`@typescript/typescript/unstable/${mode}`, benchUrl));
const { SyntaxKind } = await import(import.meta.resolve("@typescript/typescript/unstable/ast", benchUrl));
const children = trackChildren();
const work = {};
const details = {};

function digest(value) {
    return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

function path(value) { return value.replaceAll(repoRoot, "@UPSTREAM@/"); }

async function projectTask(name, action) {
    const api = new API({ cwd: repoRoot });
    try {
        const snapshot = await api.updateSnapshot({ openProject: "tsc/testdata/fixtures/compiler/tsconfig.json" });
        const project = snapshot.getProjects()[0];
        if (!project) throw new Error("benchmark project is absent");
        const values = await action(project, api);
        work[name] = { count: values.length, sha256: digest(values) };
        if (process.env.PHASE7_API_WORK_DETAIL === name) details[name] = values;
    } finally {
        try { await api.close(); }
        finally { await children.drain(); }
    }
}

try {
    await projectTask("load_snapshot", async (project, api) => {
        const again = await api.updateSnapshot({ openProject: "tsc/testdata/fixtures/compiler/tsconfig.json" });
        return [project, again.getProjects()[0]].map(p => ({
            config: path(p.configFileName), roots: p.rootFiles.map(path),
        }));
    });
    for (const fileName of ["debug.ts", "program.ts", "checker.ts"]) {
        await projectTask(`transfer_${fileName.replace(".", "_")}`, async project => {
            const file = await project.program.getSourceFile(fileName);
            if (!file) throw new Error(`missing ${fileName}`);
            const nodes = [];
            function visit(node) {
                const direct = [];
                node.forEachChild(child => { direct.push(child); });
                nodes.push([node.kind, node.pos, node.end, node.flags,
                    node.text ?? null, node.escapedText ?? null, direct.length]);
                for (const child of direct) visit(child);
            }
            visit(file);
            return nodes;
        });
    }
    await projectTask("getsymbolatposition_one_location", async project => {
        const symbol = symbolObserver(repoRoot);
        await project.checker.getSymbolAtPosition("core.ts", 0);
        return [symbol(await project.checker.getSymbolAtPosition("program.ts", 8895))];
    });
    for (const method of ["getSymbolAtPosition", "getSymbolAtLocation"]) {
        for (const batched of [false, true]) {
            const name = `${method.toLowerCase()}_identifiers${batched ? "_batched" : ""}`;
            await projectTask(name, async project => {
                const symbol = symbolObserver(repoRoot);
                await project.checker.getSymbolAtPosition("core.ts", 0);
                const file = await project.program.getSourceFile("program.ts");
                if (!file) throw new Error("missing program.ts");
                const nodes = [];
                file.forEachChild(function visit(node) {
                    if (node.kind === SyntaxKind.Identifier) nodes.push(node);
                    node.forEachChild(visit);
                });
                const query = values => method === "getSymbolAtPosition"
                    ? project.checker.getSymbolAtPosition("program.ts", values)
                    : project.checker.getSymbolAtLocation(values);
                const args = method === "getSymbolAtPosition" ? nodes.map(node => node.pos) : nodes;
                const values = [];
                if (batched) values.push(...await query(args));
                else for (const arg of args) values.push(await query(arg));
                if (values.length !== nodes.length) throw new Error(`${name}: query dropped results`);
                // Include each input's range so the same count on a different
                // tree/query schedule cannot authenticate the same answers.
                return values.map((value, i) => [nodes[i].pos, nodes[i].end, symbol(value)]);
            });
        }
    }
    writeFileSync(outFile, JSON.stringify({ work, details, lifecycle: children.report() }));
} finally {
    await children.dispose();
}
