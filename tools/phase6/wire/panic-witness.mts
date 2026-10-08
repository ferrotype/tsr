// The Phase 6 real-client panic witness (docs/PHASE6-plan.md, A5 item 2).
//
// Drives the untouched asynchronous client against a server binary. Against
// the private test server (`phase5_testserver --api`, built with the
// `fault-injection` feature) it arms a panic in the next checker operation of
// one snapshot and shows what the client then sees: the panicking request
// fails with the connection's `panic:` error, every old handle of the two
// snapshots that share the project's checker pool is rejected with the
// server's error form, a project on another pool keeps answering, and a
// fresh snapshot after a file change answers again on a new pool. Against
// the pinned Go binary (`--native`) the same script runs without the fault
// and shows the ordinary wire error forms the client already handles.
//
//   node --conditions @typescript/source tools/phase6/wire/panic-witness.mts <binary> [--native]
//
// Run from the repository root; the client package is the pin's.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// The pinned client by path, as handshake.mts loads it.
const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../upstream/packages/typescript");
const { API } = await import(pathToFileURL(path.join(packageDir, "src/api/async/api.ts")).href);

const [binary, mode] = process.argv.slice(2);
if (!binary) {
    console.error("usage: panic-witness.mts <server binary> [--native]");
    process.exit(2);
}
const native = mode === "--native";
const FAULT_METHOD = "testhost/faultNextCheckerOperation";

const root = mkdtempSync(path.join(tmpdir(), "tsr-panic-witness-"));
const p = path.join(root, "p");
const q = path.join(root, "q");
const files: Record<string, string> = {
    [path.join(p, "tsconfig.json")]: JSON.stringify({ compilerOptions: { noLib: true, strict: true } }),
    [path.join(p, "src", "index.ts")]: "export const answer = 1;\nexport const twice = answer + answer;\n",
    [path.join(q, "tsconfig.json")]: JSON.stringify({ compilerOptions: { noLib: true, strict: true } }),
    [path.join(q, "src", "other.ts")]: "export const other = 'q';\n",
};
for (const [file, text] of Object.entries(files)) {
    const dir = path.dirname(file);
    mkdirSync(dir, { recursive: true });
    writeFileSync(file, text);
}

const report: Record<string, unknown> = {};
function record(step: string, value: unknown) {
    report[step] = value;
    console.log(JSON.stringify({ step, ...(typeof value === "object" && value !== null ? value : { value }) }));
}
async function failure(work: () => Promise<unknown>): Promise<string> {
    try {
        await work();
        return "";
    }
    catch (error) {
        return error instanceof Error ? error.message.split("\n")[0] : String(error);
    }
}

const api = new API({ tsserverPath: binary, cwd: root });
try {
    // Two snapshots sharing project p's checker pool, and project q on its own.
    const s1 = await api.updateSnapshot({ openProjects: [path.join(p, "tsconfig.json"), path.join(q, "tsconfig.json")] });
    const s2 = await api.updateSnapshot();
    const indexFile = path.join(p, "src", "index.ts");
    const otherFile = path.join(q, "src", "other.ts");
    const project1 = s1.getProject(path.join(p, "tsconfig.json"))!;
    const project2 = s2.getProject(path.join(p, "tsconfig.json"))!;
    const projectQ = s2.getProject(path.join(q, "tsconfig.json"))!;
    const symbol1 = (await project1.checker.getSymbolAtPosition(indexFile, 13))!;
    const symbol2 = (await project2.checker.getSymbolAtPosition(indexFile, 13))!;
    const symbolQ = (await projectQ.checker.getSymbolAtPosition(otherFile, 13))!;
    record("before", {
        snapshots: [s1.id, s2.id],
        symbols: [symbol1.name, symbol2.name, symbolQ.name],
    });

    if (!native) {
        // The fault: snapshot 2's next checker operation panics inside its
        // operation, so the lease retires the pool generation (ADR 0012).
        const armed = await (api as any).client.apiRequest(FAULT_METHOD, { snapshot: s2.id });
        record("armed", { armed });
        const faulted = await failure(() => project2.checker.getTypeOfSymbol(symbol2));
        record("faulted request", { error: faulted, isPanic: faulted.startsWith("panic:") });
        // Both snapshots' old handles are rejected: the generation is retired.
        const rejected1 = await failure(() => project1.checker.getTypeOfSymbol(symbol1));
        const rejected2 = await failure(() => project2.checker.getTypeOfSymbol(symbol2));
        record("old handles after the fault", { snapshot1: rejected1, snapshot2: rejected2 });
    }
    else {
        // Without a fault the handles keep answering; a released snapshot's
        // handle shows the error form the client gets for a gone snapshot.
        const type1 = await project1.checker.getTypeOfSymbol(symbol1);
        record("old handles", { snapshot1: type1.flags });
    }

    // The other project's pool keeps answering.
    const typeQ = await projectQ.checker.getTypeOfSymbol(symbolQ);
    record("other pool", { flags: typeQ.flags });

    // A file change rebuilds project p's program on a fresh pool.
    writeFileSync(indexFile, "export const answer = 2;\nexport const twice = answer + answer;\n");
    const s3 = await api.updateSnapshot({ fileChanges: { changed: [indexFile] } });
    const project3 = s3.getProject(path.join(p, "tsconfig.json"))!;
    const symbol3 = (await project3.checker.getSymbolAtPosition(indexFile, 13))!;
    const type3 = await project3.checker.getTypeOfSymbol(symbol3);
    record("fresh snapshot", { snapshot: s3.id, symbol: symbol3.name, flags: type3.flags, idsDistinct: s3.id !== s1.id && s3.id !== s2.id });

    // A released snapshot's handle: the client-visible error form.
    await s1.dispose();
    const released = await failure(() => project1.checker.getTypeOfSymbol(symbol1));
    record("released snapshot", { error: released });
}
finally {
    await api.close();
}

// A reconnect answers on the same project files.
const api2 = new API({ tsserverPath: binary, cwd: root });
try {
    const s = await api2.updateSnapshot({ openProjects: [path.join(p, "tsconfig.json")] });
    const project = s.getProject(path.join(p, "tsconfig.json"))!;
    const symbol = (await project.checker.getSymbolAtPosition(path.join(p, "src", "index.ts"), 13))!;
    record("reconnect", { snapshot: s.id, symbol: symbol.name });
}
finally {
    await api2.close();
    rmSync(root, { recursive: true, force: true });
}

// The verdict checks what the record claims, step by step.
const checks: Record<string, boolean> = {};
const faulted = report["faulted request"] as { isPanic?: boolean } | undefined;
const after = report["old handles after the fault"] as { snapshot1?: string; snapshot2?: string } | undefined;
const fresh = report["fresh snapshot"] as { idsDistinct?: boolean; symbol?: string } | undefined;
const released = report["released snapshot"] as { error?: string } | undefined;
const reconnect = report["reconnect"] as { symbol?: string } | undefined;
const retired = (error: string | undefined) => !!error && error.includes("the checker generation has retired");
if (!native) {
    checks["the faulted request fails with the connection's panic error"] = faulted?.isPanic === true;
    checks["snapshot 1's old handle is rejected as retired"] = retired(after?.snapshot1);
    checks["snapshot 2's old handle is rejected as retired"] = retired(after?.snapshot2);
}
else {
    checks["old handles keep answering without a fault"] = report["old handles"] !== undefined;
}
checks["the other project's pool keeps answering"] = report["other pool"] !== undefined;
checks["a fresh snapshot answers on a new id"] = fresh?.idsDistinct === true && fresh?.symbol === "answer";
checks["a released snapshot's handle reports the snapshot as gone"] = !!released?.error && released.error.includes("not found");
checks["a reconnect answers"] = reconnect?.symbol === "answer";
const failed = Object.entries(checks).filter(([, passed]) => !passed).map(([name]) => name);
const ok = failed.length === 0;
console.log(JSON.stringify({ step: "verdict", ok, failed }));
process.exit(ok ? 0 : 1);
