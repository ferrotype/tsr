import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { createInstance } from "./instance.mjs";
import { checkImports } from "./imports.mjs";

const directory = resolve(process.argv[2] ?? "target/s10/corpus");
const { createBindings } = await import(pathToFileURL(`${directory}/bindings.mjs`));
const module = new WebAssembly.Module(readFileSync(`${directory}/corpus_bg.wasm`));
checkImports(module, "corpus");
let instance = createInstance(createBindings, module);
for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  let output;
  try {
    // Keep the original serialized map order. JSON.parse/stringify would
    // reorder integer-looking property names before Rust receives them.
    output = instance.observeCorpusJSON(line);
  } catch (error) {
    const request = JSON.parse(line);
    output = JSON.stringify({
      version: 1, id: request.id, acceptance_tier: request.acceptance_tier,
      fatal: { state: "failed", class: "wasm_trap", reason: String(error) },
    });
    instance.dispose();
    instance = createInstance(createBindings, module);
  }
  await new Promise((resolve, reject) => process.stdout.write(output + "\n", error => error ? reject(error) : resolve()));
}
instance.dispose();
