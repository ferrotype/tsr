//! NDJSON transport to production tsr_wasm single-file exports. No native
//! compiler, corpus adapter or fallback participates in a response.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { createInstance } from "../../s10/wasm/instance.mjs";
import { checkImports } from "../../s10/wasm/imports.mjs";

const decode = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const requestFields = ["version", "declaration", "source", "filename", "compiler_options", "report_diagnostics"];
const responseFields = ["version", "output", "source_map", "diagnostics", "sources"];
const diagnosticFields = ["file", "pos", "end", "code", "category", "source", "key", "text", "args", "display_args", "has_message", "ad_hoc_message", "chain", "related", "reports_unnecessary", "reports_deprecated", "skipped_on_no_emit", "repopulate"];

function object(value, fields, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`);
  }
  if (Object.keys(value).length !== fields.length || !fields.every(field => Object.hasOwn(value, field))) {
    throw new TypeError(`${label} has missing or unknown fields`);
  }
}

function bytes(value, label) {
  if (!Array.isArray(value) || !value.every(byte => Number.isInteger(byte) && byte >= 0 && byte <= 255)) {
    throw new TypeError(`${label} must be a byte array`);
  }
}

function boolean(value, label) {
  if (typeof value !== "boolean") throw new TypeError(`${label} must be a boolean`);
}

function array(value, label) {
  if (!Array.isArray(value)) throw new TypeError(`${label} must be an array`);
}

function integer(value, minimum, maximum, label) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`${label} must be an integer in [${minimum}, ${maximum}]`);
  }
}

// JSON.parse checks syntax but silently replaces duplicate names. Scan its
// already validated token boundaries to reject duplicates, including escaped
// spellings of one name, without reserializing ordered options or responses.
function parseJSON(text, label) {
  const value = JSON.parse(text);
  const containers = [];
  for (let index = 0; index < text.length; index++) {
    const char = text[index];
    if (char === "{") containers.push(new Set());
    else if (char === "[") containers.push(null);
    else if (char === "}" || char === "]") containers.pop();
    else if (char === '"') {
      const start = index++;
      while (text[index] !== '"') {
        if (text[index] === "\\") index++;
        index++;
      }
      let next = index + 1;
      while (/\s/.test(text[next] ?? "")) next++;
      if (text[next] === ":") {
        const key = JSON.parse(text.slice(start, index + 1));
        const names = containers.at(-1);
        if (names.has(key)) throw new TypeError(`${label} contains duplicate field ${key}`);
        names.add(key);
      }
    }
  }
  return value;
}

function validateRequest(request) {
  object(request, requestFields, "transpile request");
  if (request.version !== 1) throw new TypeError(`unsupported portable transpile version: ${request.version}`);
  boolean(request.declaration, "declaration");
  boolean(request.report_diagnostics, "report_diagnostics");
  for (const field of ["source", "filename", "compiler_options"]) bytes(request[field], field);
  if (request.source.length > 2147483647 || request.filename.length > 2147483647) {
    throw new RangeError("source and filename must fit signed source positions");
  }
}

function validateDiagnostic(value, sources) {
  object(value, diagnosticFields, "diagnostic");
  if (value.file !== null) integer(value.file, 0, sources - 1, "diagnostic file");
  for (const field of ["pos", "end"]) integer(value[field], Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER, field);
  for (const field of ["code", "category"]) integer(value[field], -2147483648, 2147483647, field);
  for (const field of ["source", "key", "text"]) bytes(value[field], field);
  for (const field of ["args", "display_args"]) {
    array(value[field], field);
    for (const argument of value[field]) bytes(argument, field);
  }
  for (const field of ["has_message", "reports_unnecessary", "reports_deprecated", "skipped_on_no_emit"]) boolean(value[field], field);
  if (value.ad_hoc_message !== null) bytes(value.ad_hoc_message, "ad_hoc_message");
  for (const field of ["chain", "related"]) {
    array(value[field], field);
    for (const diagnostic of value[field]) validateDiagnostic(diagnostic, sources);
  }
  if (value.repopulate !== null) {
    object(value.repopulate, ["kind", "module_reference", "mode", "package_name"], "repopulate");
    for (const field of ["kind", "mode"]) integer(value.repopulate[field], -2147483648, 2147483647, field);
    for (const field of ["module_reference", "package_name"]) bytes(value.repopulate[field], field);
  }
}

function validateResponse(response) {
  object(response, responseFields, "transpile response");
  if (response.version !== 1) throw new TypeError(`unsupported portable transpile response version: ${response.version}`);
  for (const field of ["output", "source_map"]) bytes(response[field], field);
  array(response.sources, "sources");
  for (const source of response.sources) {
    object(source, ["name", "path", "text", "canonical_name"], "source");
    for (const field of ["name", "path", "text"]) bytes(source[field], field);
    if (source.canonical_name !== null) throw new TypeError("mapped and supplemental sources are unsupported");
  }
  array(response.diagnostics, "diagnostics");
  for (const diagnostic of response.diagnostics) validateDiagnostic(diagnostic, response.sources.length);
}

export function createProcessor(createBindings, module) {
  const instance = createInstance(createBindings, module);
  return Object.freeze({
    processLine(line) {
      const request = parseJSON(line, "transpile request");
      validateRequest(request);
      // Decode only option JSON. Source and filename remain exact bytes.
      const options = decode.decode(Uint8Array.from(request.compiler_options));
      const method = request.declaration ? "transpileDeclaration" : "transpileModule";
      const result = instance[method](Uint8Array.from(request.source), Uint8Array.from(request.filename), options, request.report_diagnostics);
      if (!(result instanceof Uint8Array)) throw new TypeError("wasm transpile response must be owned bytes");
      const json = decode.decode(result);
      validateResponse(parseJSON(json, "transpile response"));
      // Retain the exact result text. Parsing/stringifying could reorder maps
      // or round integer fields; the host independently decodes this record.
      return json;
    },
    dispose() { instance.dispose(); },
    get state() { return instance.state; },
  });
}

export async function loadProcessor(directory) {
  if (typeof directory !== "string" || directory.length === 0) {
    throw new Error("TSR_WASM_ARTIFACT_DIR must name the production checker build directory");
  }
  directory = resolve(directory);
  const record = JSON.parse(readFileSync(resolve(directory, "build.json"), "utf8"));
  if (record.mode !== "checker" || !Array.isArray(record.command) ||
      !record.command.some((part, index) => part === "-p" && record.command[index + 1] === "tsr_wasm") ||
      !record.command.some((part, index) => part === "--features" && record.command[index + 1].split(",").includes("checker"))) {
    throw new Error("artifact build record is not the production tsr_wasm checker build");
  }
  const files = new Map();
  for (const name of ["checker_bg.wasm", "bindings.mjs"]) {
    const data = readFileSync(resolve(directory, name));
    const entries = Object.entries(record.artifacts ?? {}).filter(([path]) => path.split("/").at(-1) === name);
    if (entries.length !== 1 || entries[0][1].sha256 !== createHash("sha256").update(data).digest("hex")) {
      throw new Error(`artifact bytes differ from build record: ${name}`);
    }
    files.set(name, data);
  }
  const module = new WebAssembly.Module(files.get("checker_bg.wasm"));
  checkImports(module, "checker");
  const names = new Set(WebAssembly.Module.exports(module).filter(value => value.kind === "function").map(value => value.name));
  for (const name of ["transpile_module", "transpile_declaration"]) {
    if (!names.has(name)) throw new Error(`production wasm export is missing: ${name}`);
  }
  const { createBindings } = await import(pathToFileURL(resolve(directory, "bindings.mjs")));
  if (typeof createBindings !== "function") throw new TypeError("artifact does not export createBindings");
  return createProcessor(createBindings, module);
}

export async function consume(input, output, processor) {
  try {
    for await (const line of createInterface({ input, crlfDelay: Infinity })) {
      const result = processor.processLine(line);
      await new Promise((resolve, reject) => output.write(result + "\n", error => error ? reject(error) : resolve()));
    }
  } finally {
    processor.dispose();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 2) throw new Error("wasm consumer takes no arguments; set TSR_WASM_ARTIFACT_DIR");
    await consume(process.stdin, process.stdout, await loadProcessor(process.env.TSR_WASM_ARTIFACT_DIR));
  } catch (error) {
    process.stderr.write(`wasm transpile consumer: ${error}\n`);
    process.exitCode = 1;
  }
}
