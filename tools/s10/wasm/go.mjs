import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";

export async function goParser(directory = "target/s10/go") {
  createRequire(import.meta.url)(resolve(directory, "wasm_exec.cjs"));
  const go = new globalThis.Go();
  const module = new WebAssembly.Module(readFileSync(resolve(directory, "parser.wasm")));
  const instance = new WebAssembly.Instance(module, go.importObject);
  let failure;
  void go.run(instance).catch(error => { failure = error; });
  if (failure) throw failure;
  if (!globalThis.s10Parser) throw new Error("Go parser did not register its entry points");
  return globalThis.s10Parser;
}
