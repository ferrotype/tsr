// The LSP-hosted API session handshake with the untouched asynchronous client.
//
// Starts `<binary> --lsp --stdio`, initializes it over JSON-RPC, asks for
// `custom/initializeAPISession`, connects the pinned client to the announced
// socket with `API.fromLSPConnection({ pipe })`, runs one query and prints
// every observation as one JSON line, so the same script run against the
// pin's binary and against `tsrust` can be compared line by line.
//
//   node --conditions @typescript/source tools/phase6/wire/handshake.mts <lsp binary>
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// The pinned client by path: the package's own `#vscode-jsonrpc` import
// resolves from its package.json wherever the importing script lives.
const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../upstream/packages/typescript");
const { API } = await import(pathToFileURL(path.join(packageDir, "src/api/async/api.ts")).href);

const binary = process.argv[2];
if (!binary) {
    console.error("usage: handshake.mts <lsp binary>");
    process.exit(2);
}

const server = spawn(binary, ["--lsp", "--stdio"], { stdio: ["pipe", "pipe", "pipe"] });
let stderr = "";
server.stderr.on("data", chunk => (stderr += chunk));

// A minimal JSON-RPC reader over Content-Length frames.
let buffer = Buffer.alloc(0);
const pending = new Map<number, (message: any) => void>();
server.stdout.on("data", chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
        const headerEnd = buffer.indexOf("\r\n\r\n");
        if (headerEnd < 0) return;
        const header = buffer.subarray(0, headerEnd).toString();
        const match = /Content-Length: (\d+)/i.exec(header);
        if (!match) throw new Error(`bad header: ${header}`);
        const length = Number(match[1]);
        if (buffer.length < headerEnd + 4 + length) return;
        const body = buffer.subarray(headerEnd + 4, headerEnd + 4 + length).toString();
        buffer = buffer.subarray(headerEnd + 4 + length);
        const message = JSON.parse(body);
        if (typeof message.method === "string") {
            // A server-to-client request or notification: answer requests
            // with null so the server never waits on this client.
            if (message.id !== undefined) {
                const reply = JSON.stringify({ jsonrpc: "2.0", id: message.id, result: null });
                server.stdin.write(`Content-Length: ${Buffer.byteLength(reply)}\r\n\r\n${reply}`);
            }
            report("server-message", message.method);
        } else if (typeof message.id === "number" && pending.has(message.id)) {
            pending.get(message.id)!(message);
            pending.delete(message.id);
        }
    }
});

let nextId = 1;
function request(method: string, params?: unknown): Promise<any> {
    const id = nextId++;
    const body = JSON.stringify(params === undefined ? { jsonrpc: "2.0", id, method } : { jsonrpc: "2.0", id, method, params });
    server.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
    return new Promise(resolve => pending.set(id, resolve));
}
function notify(method: string, params?: unknown) {
    const body = JSON.stringify(params === undefined ? { jsonrpc: "2.0", method } : { jsonrpc: "2.0", method, params });
    server.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
}
function report(name: string, value: unknown) {
    console.log(JSON.stringify({ [name]: value }));
}

const timeout = setTimeout(() => {
    report("error", "timed out");
    server.kill();
    process.exit(1);
}, 30_000);

try {
    const initialized = await request("initialize", {
        processId: process.pid,
        rootUri: null,
        capabilities: {},
        initializationOptions: {},
    });
    report("lsp-initialize", initialized.error ?? "ok");
    notify("initialized", {});

    const session = await request("custom/initializeAPISession", {});
    if (session.error) {
        report("initializeAPISession", session.error);
        throw new Error("no session");
    }
    const { sessionId, pipe } = session.result;
    report("initializeAPISession", {
        sessionIdShape: /^api-session-\d+$/.test(sessionId),
        pipeShape: /tsgo-api-[0-9a-f]+-[0-9a-f]+$/.test(pipe),
    });

    const api = await API.fromLSPConnection({ pipe });
    report("api-initialized", true);
    const timing = await api.getTimingInfo();
    // The server's half of the snapshot: collection is off for LSP-hosted
    // sessions, so the shape is what counts.
    report("getTimingInfo", { server: timing.server });
    let unknownError = "none";
    try {
        await (api as any).client.apiRequest("noSuchMethod", {});
    } catch (error: any) {
        unknownError = String(error?.message ?? error);
    }
    report("unknown-method", unknownError);
    await api.close();
    report("api-closed", true);

    const second = await request("custom/initializeAPISession", { pipe: `${pipe}-second` });
    report("second-session", {
        sessionIdDiffers: second.result?.sessionId !== sessionId,
        pipeHonored: second.result?.pipe === `${pipe}-second`,
    });
    const another = await API.fromLSPConnection({ pipe: `${pipe}-second` });
    await another.close();
    report("second-session-connected", true);

    const shutdown = await request("shutdown");
    report("lsp-shutdown", shutdown.error ?? shutdown.result ?? null);
    // Whether the process ends is compared; its exit code is not: the pin's
    // LSP command exits 1 after a shutdown-then-exit sequence and the Rust
    // one exits 0, a Phase 5 observation outside the API session's scope.
    const exited = new Promise<string>(resolve => server.on("exit", code => resolve(`exited (code ${code})`)));
    notify("exit");
    server.stdin.end();
    const outcome = await Promise.race([
        exited,
        new Promise<string>(resolve => setTimeout(() => resolve("still running 5 s after exit"), 5_000)),
    ]);
    report("lsp-exit", outcome.startsWith("exited") ? "exited" : outcome);
    if (!outcome.startsWith("exited")) server.kill();
} catch (error: any) {
    report("error", String(error?.message ?? error));
    report("stderr", stderr.slice(-2000));
    server.kill();
    process.exitCode = 1;
} finally {
    clearTimeout(timeout);
}
