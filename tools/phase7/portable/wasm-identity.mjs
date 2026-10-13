import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const digest = data => createHash("sha256").update(data).digest("hex");

// Called only after the loader verifies the build record, artifact bytes and
// independent ABI policy. Paths are omitted so downloaded shards can agree.
export function wasmIdentity(family, recordBytes, files) {
  return {
    version: 1, runtime: "wasm", family,
    engine: { version: process.version, flags: process.execArgv, node_options: process.env.NODE_OPTIONS ?? null,
      sha256: digest(readFileSync(process.execPath)) },
    build_sha256: digest(recordBytes),
    artifacts: Object.fromEntries([...files].map(([name, data]) => [name, digest(data)])),
  };
}
