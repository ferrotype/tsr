// Minimal client for the pin's real --api --async JSON-RPC socket transport.
// No alternate parser server or mocked parse endpoint.
import net from "node:net";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

export async function socketServer(executable, cwd, pipe) {
  const server = spawn(executable, ["--api", "--async", "--cwd", cwd, "--pipe", pipe], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  let startupError;
  server.stderr.on("data", data => { stderr += data.toString(); });
  server.on("error", error => { startupError = error; });
  let socket;
  try {
    for (let attempt = 0; attempt < 200; attempt++) {
      if (startupError) throw startupError;
      if (server.exitCode !== null) throw new Error(`API server exited: ${stderr}`);
      try {
        socket = await new Promise((resolve, reject) => {
          const candidate = net.createConnection(pipe);
          candidate.once("error", error => { candidate.destroy(); reject(error); });
          candidate.once("connect", () => { candidate.removeAllListeners("error"); resolve(candidate); });
        });
        break;
      } catch (error) {
        if (!["ENOENT", "ECONNREFUSED"].includes(error.code)) throw error;
        await delay(25);
      }
    }
    if (!socket) throw new Error(`API server did not open its socket: ${stderr}`);
  } catch (error) {
    server.kill();
    throw error;
  }
  let next = 0;
  let buffer = Buffer.alloc(0);
  const pending = new Map();
  const rejectAll = error => {
    for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(error); }
    pending.clear();
  };
  socket.on("error", rejectAll);
  socket.on("close", () => rejectAll(new Error("API socket closed")));
  socket.on("data", data => {
    buffer = Buffer.concat([buffer, data]);
    try {
      for (;;) {
        const end = buffer.indexOf("\r\n\r\n");
        if (end < 0) break;
        const header = buffer.subarray(0, end).toString("ascii");
        const matched = /^Content-Length: (\d+)$/mi.exec(header);
        if (!matched) throw new Error("missing JSON-RPC content length");
        const length = Number(matched[1]);
        if (buffer.length < end + 4 + length) break;
        const response = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString("utf8"));
        buffer = buffer.subarray(end + 4 + length);
        const entry = pending.get(response.id);
        if (!entry) throw new Error(`unsolicited API response: ${JSON.stringify(response)}`);
        pending.delete(response.id);
        clearTimeout(entry.timer);
        if (response.error) entry.reject(new Error(JSON.stringify(response.error)));
        else entry.resolve(response.result);
      }
    } catch (error) { rejectAll(error); socket.destroy(); }
  });
  return {
    call(method, params = {}) {
      const id = ++next;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`API timeout: ${method}`));
        }, 30000);
        pending.set(id, { resolve, reject, timer });
        const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
        socket.write(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]));
      });
    },
    async close() {
      socket.destroy();
      server.kill();
      if (server.exitCode === null) await new Promise(resolve => server.once("exit", resolve));
    },
    get stderr() { return stderr; },
  };
}
