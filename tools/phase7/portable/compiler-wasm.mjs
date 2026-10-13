//! Test-only public tsr_embed consumer. The exact native Rust executor, DTOs
//! and walker run in wasm; this transport performs no compiler observations.
import { createHash } from "node:crypto";
import { readFileSync, readSync, writeSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { checkCompilerImports } from "./compiler-imports.mjs";
import { wasmIdentity } from "./wasm-identity.mjs";

const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

// Nested callback replies must be read while the Rust execute call is suspended
// inside its synchronous import. An async readline loop cannot serve that ABI.
export function lineReader(read = buffer => readSync(0, buffer)) {
  let pending = Buffer.alloc(0);
  let ended = false;
  return () => {
    for (;;) {
      const newline = pending.indexOf(10);
      if (newline >= 0) {
        const line = pending.subarray(0, newline);
        pending = pending.subarray(newline + 1);
        return decoder.decode(line);
      }
      if (ended) {
        if (pending.length === 0) return null;
        const line = pending;
        pending = Buffer.alloc(0);
        return decoder.decode(line);
      }
      const chunk = Buffer.allocUnsafe(65536);
      const count = read(chunk);
      if (count === 0) ended = true;
      else pending = Buffer.concat([pending, chunk.subarray(0, count)]);
    }
  };
}

export function lineWriter(write = buffer => writeSync(1, buffer)) {
  return line => {
    const bytes = Buffer.from(line + "\n");
    let offset = 0;
    while (offset < bytes.length) {
      const count = write(bytes.subarray(offset));
      if (!Number.isInteger(count) || count <= 0 || count > bytes.length - offset) {
        throw new Error("compiler frame write made invalid progress");
      }
      offset += count;
    }
  };
}

export function createCompilerProcessor(createBindings, module, callback) {
  const bindings = createBindings();
  bindings.initSync({ module });
  const compiler = new bindings.Compiler();
  let state = "live";
  return Object.freeze({
    processLine(line) {
      if (state !== "live") throw new Error(`compiler wasm instance is ${state}`);
      if (Object.hasOwn(globalThis, "tsrPortableCompilerCallback")) {
        throw new Error("compiler wasm callback reentry is prohibited");
      }
      globalThis.tsrPortableCompilerCallback = callback;
      try {
        // Rust validates the original JSON, including version, unknown fields,
        // byte arrays, integer ranges and duplicate DTO fields. Never parse and
        // reserialize the request: its options and numeric IDs remain exact.
        const result = compiler.execute(line);
        if (typeof result !== "string") throw new TypeError("compiler frame must be a JSON string");
        const frame = JSON.parse(result);
        if (frame?.kind !== "response" && frame?.kind !== "failure") {
          throw new TypeError("compiler export returned an invalid outer frame");
        }
        if (frame.kind === "failure") state = "failed";
        return result;
      } catch (error) {
        state = "failed";
        throw error;
      } finally {
        delete globalThis.tsrPortableCompilerCallback;
      }
    },
    dispose() {
      // A trapped module is never reentered, including through a destructor.
      if (state === "live") compiler.free();
      state = "disposed";
    },
    get state() { return state; },
  });
}

export async function loadCompilerProcessor(directory, callback, identityOnly = false) {
  if (typeof directory !== "string" || directory.length === 0) {
    throw new Error("TSR_WASM_COMPILER_ARTIFACT_DIR must name the test-only public embedding wasm build");
  }
  directory = resolve(directory);
  const recordBytes = readFileSync(resolve(directory, "build.json"));
  const record = JSON.parse(recordBytes);
  if (record.mode !== "phase7-test-compiler" || record.package !== "phase7_wasm_compiler" || record.version !== 1 || record.performance_eligible !== false) {
    throw new Error("artifact is not the test-only public embedding compiler build");
  }
  const files = new Map();
  for (const name of ["compiler_bg.wasm", "bindings.mjs"]) {
    const data = readFileSync(resolve(directory, name));
    const expected = record.artifacts?.[name];
    if (expected?.sha256 !== createHash("sha256").update(data).digest("hex") || expected?.bytes !== data.length) {
      throw new Error(`artifact bytes differ from build record: ${name}`);
    }
    files.set(name, data);
  }
  const module = new WebAssembly.Module(files.get("compiler_bg.wasm"));
  const imports = checkCompilerImports(module);
  if (JSON.stringify(imports) !== JSON.stringify(record.imports)) {
    throw new Error("compiler wasm imports differ from the build record");
  }
  if (identityOnly) return wasmIdentity("compiler", recordBytes, files);
  const { createBindings } = await import(pathToFileURL(resolve(directory, "bindings.mjs")));
  return createCompilerProcessor(createBindings, module, callback);
}

export function consumeCompiler(processor, receive, send) {
  try {
    for (;;) {
      const line = receive();
      if (line === null) return;
      const result = processor.processLine(line);
      send(result);
      if (processor.state !== "live") throw new Error("compiler wasm command failed; see failure frame");
    }
  } finally {
    processor.dispose();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length === 3 && process.argv[2] === "--identity") {
      process.stdout.write(JSON.stringify(await loadCompilerProcessor(process.env.TSR_WASM_COMPILER_ARTIFACT_DIR, undefined, true)) + "\n");
    } else {
      if (process.argv.length !== 2) throw new Error("compiler wasm consumer takes no arguments");
      const receive = lineReader();
      const send = lineWriter();
      const callback = request => {
        send(`{"kind":"callback","value":${request}}`);
        const reply = receive();
        if (reply === null) throw new Error("compiler callback reply ended at EOF");
        return reply;
      };
      consumeCompiler(await loadCompilerProcessor(process.env.TSR_WASM_COMPILER_ARTIFACT_DIR, callback), receive, send);
    }
  } catch (error) {
    process.stderr.write(`wasm compiler consumer: ${error}\n`);
    process.exitCode = 1;
  }
}
