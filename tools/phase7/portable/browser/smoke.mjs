import { createBindings } from "/artifact/bindings.mjs";
import { createInstanceAsync, WasmApiError } from "/runtime/instance.mjs";
import { checkImports } from "/runtime/imports.mjs";

const bytes = value => new TextEncoder().encode(value);
const text = value => new TextDecoder("utf-8", { fatal: true }).decode(value);
const json = value => JSON.parse(text(value));
function assert(value, reason) { if (!value) throw new Error(reason); }
function equal(value, expected, reason) {
  assert(JSON.stringify(value) === JSON.stringify(expected), `${reason}: ${JSON.stringify(value)}`);
}
function throws(operation, match) {
  let caught;
  try { operation(); } catch (error) { caught = error; }
  assert(caught !== undefined && match(caught), "expected operation failure");
  return caught;
}

export async function runSmoke() {
  const binary = await (await fetch("/artifact/checker_bg.wasm")).arrayBuffer();
  // Chromium caps synchronous main-thread compilation at 8 MiB. Compile the
  // production artifact asynchronously; do not raise that engine limit.
  const module = await WebAssembly.compile(binary);
  const imports = checkImports(module, "checker");
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", binary)), byte => byte.toString(16).padStart(2, "0")).join("");
  const result = { version: 1, user_agent: navigator.userAgent, wasm_sha256: digest, imports, cases: [] };
  const run = async (id, operation) => {
    try { result.cases.push({ id, state: "passed", observations: await operation() }); }
    catch (error) { result.cases.push({ id, state: "failed", reason: `${String(error)}\n${error.stack ?? ""}` }); }
  };

  await run("parse-and-owned-encode", async () => {
    const instance = await createInstanceAsync(createBindings, module);
    try {
      const counts = Array.from(instance.parse(bytes("const n = 1;"), bytes("/parse.ts"), 3));
      assert(counts[0] > 0 && counts[2] === 0, "parse counts and diagnostics");
      const encoded = instance.parseAndEncode(bytes("const n = 1;"), bytes("/parse.ts"), 3);
      assert(encoded.length > 0, "encoded AST is nonempty");
      const saved = Array.from(encoded);
      instance.parse(bytes("const next = 2;"), bytes("/next.ts"), 3);
      instance.dispose();
      equal(Array.from(encoded), saved, "owned encoded bytes survive disposal");
      return { counts, encoded_bytes: encoded.length };
    } finally { instance.dispose(); }
  });

  await run("memory-host-check-and-api-errors", async () => {
    const instance = await createInstanceAsync(createBindings, module);
    try {
      const host = instance.createHost(bytes("/"));
      host.addFile(bytes("/check.ts"), bytes("const value: string = 1;"), true);
      const session = host.compile({ strict: true, noLib: true });
      try {
        const diagnostics = session.diagnostics();
        assert(diagnostics.some(value => value.code === 2322), "type mismatch diagnostic 2322");
        equal(text(session.typeAtPosition(bytes("/check.ts"), 6)), "string", "type query");
        throws(() => session.typeAtPosition(bytes("/missing.ts"), 0), error => error instanceof WasmApiError);
        equal(instance.state, "live", "explicit API error leaves instance live");
        equal(text(session.typeAtPosition(bytes("/check.ts"), 6)), "string", "query after API error");
        session.retire();
        throws(() => session.diagnostics(), error => error instanceof WasmApiError);
        equal(instance.state, "live", "session retirement leaves instance live");
        const traceHost = instance.createHost(bytes("/"));
        traceHost.addFile(bytes("/trace-main.ts"), bytes('import { value } from "./trace-dep"; export const result = value;'), true);
        traceHost.addFile(bytes("/trace-dep.ts"), bytes("export const value = 1;"));
        const traceSession = traceHost.compile({ noLib: true, module: 99, target: 9, traceResolution: true });
        let trace;
        try {
          trace = traceSession.resolutionTrace();
          assert(trace.length > 0, "actual module resolution trace");
          const start = trace.findIndex(row => text(Uint8Array.from(row.message_key)) === "Resolving_module_0_from_1_6086");
          const resolved = trace.findIndex(row => text(Uint8Array.from(row.message_key)) === "Module_name_0_was_successfully_resolved_to_1_6089");
          assert(start >= 0 && resolved > start, "resolution trace order and pinned message identity");
          equal(trace[start].args, [{ kind: "text", value: Array.from(bytes("./trace-dep")) }, { kind: "text", value: Array.from(bytes("/trace-main.ts")) }], "typed resolution input arguments");
          equal(trace[resolved].args, [{ kind: "text", value: Array.from(bytes("./trace-dep")) }, { kind: "text", value: Array.from(bytes("/trace-dep.ts")) }], "typed resolved file arguments");
          equal(traceSession.diagnostics().filter(value => value.file !== null), [], "resolved import source diagnostics");
        } finally { traceSession.dispose(); }
        return { codes: diagnostics.map(value => value.code), type: "string", api_error_recovery: true, resolution_trace: trace };
      } finally { session.dispose(); }
    } finally { instance.dispose(); }
  });

  await run("memory-host-javascript-and-declaration-emit", async () => {
    const instance = await createInstanceAsync(createBindings, module);
    try {
      const host = instance.createHost(bytes("/"));
      host.addFile(bytes("/emit.ts"), bytes("export const value: number = 1;\n"), true);
      const session = host.compile({ noLib: true, declaration: true, module: 99, target: 9 });
      try {
        const output = session.emit();
        equal(output.emit_skipped, false, "emit is not skipped");
        equal(output.diagnostics, [], "emit diagnostics");
        equal(output.files.map(value => text(value.name)), ["/emit.js", "/emit.d.ts"], "emitted names");
        equal(text(output.files[0].text), "export const value = 1;\n", "JavaScript");
        equal(text(output.files[1].text), "export declare const value: number;\n", "declarations");
        throws(() => session.emit({ emitOnly: 3 }), error => error instanceof WasmApiError);
        equal(instance.state, "live", "invalid emit request leaves instance live");
        return { files: output.files.map(value => ({ name: text(value.name), bytes: value.text.length })) };
      } finally { session.dispose(); }
    } finally { instance.dispose(); }
  });

  await run("single-file-module-and-declaration-transpile", async () => {
    const instance = await createInstanceAsync(createBindings, module);
    try {
      const moduleOutput = json(instance.transpileModule(bytes("export const n: number = 1;\n"), bytes("/module.ts"), '{"sourceMap":true,"noEmit":true}', true));
      equal(moduleOutput.version, 1, "transpile response version");
      equal(text(Uint8Array.from(moduleOutput.output)), "export const n = 1;\n//# sourceMappingURL=module.js.map", "module transpile overrides noEmit");
      assert(moduleOutput.source_map.length > 0, "module source map");
      equal(moduleOutput.diagnostics, [], "module diagnostics");
      const source = "export const sym = Symbol();\nexport function f() { return 1; }\n";
      const declaration = json(instance.transpileDeclaration(bytes(source), bytes("/declaration.ts"), '{"target":2,"noEmit":true}', false));
      equal(text(Uint8Array.from(declaration.output)), "export declare const sym: unique symbol;\nexport declare function f(): number;\n", "declaration transpile");
      equal(declaration.diagnostics.map(value => value.code), [9010], "isolation error is reported without reportDiagnostics");
      assert(declaration.sources.some(value => text(Uint8Array.from(value.name)).startsWith("/lib/")), "synthetic declaration library");
      equal(text(Uint8Array.from(declaration.sources[declaration.diagnostics[0].file].text)), source, "diagnostic source bytes");
      return { module_source_map_bytes: moduleOutput.source_map.length, declaration_codes: [9010] };
    } finally { instance.dispose(); }
  });

  await run("bounded-nested-input", async () => {
    const fixtures = await (await fetch("/fixtures/parser.json")).json();
    const fixture = fixtures.find(value => value.id === "nested");
    assert(fixture !== undefined, "committed S10 nested fixture");
    const source = Uint8Array.from(fixture.source);
    const instance = await createInstanceAsync(createBindings, module);
    try {
      const counts = Array.from(instance.parse(source, Uint8Array.from(fixture.name), fixture.kind));
      equal(counts[2], 0, "nested parse diagnostics");
      assert(instance.parseAndEncode(source, Uint8Array.from(fixture.name), fixture.kind).length > 0, "nested encoding");
      const host = instance.createHost(bytes("/"));
      host.addFile(Uint8Array.from(fixture.name), source, true);
      const session = host.compile({ noLib: true, target: 9 });
      try {
        equal(session.diagnostics().filter(value => value.file !== null), [], "nested source diagnostics");
        const output = session.emit();
        assert(!output.emit_skipped && output.files.length === 1 && output.files[0].text.length > 0, "nested source emits");
        // The same source family as the ADR 0011 native transform contract:
        // crates/tsr_compiler/tests/t3_contracts.rs's left binary expression.
        // This is one focused depth witness, not that contract's whole matrix.
        const terms = 5001;
        const chain = Array(terms).fill("a").join(" + ");
        const deepSource = bytes(`declare const a: number;\nexport const x: number = ${chain};\n`);
        const deepName = bytes("/deep-left-addition.ts");
        const deep = await createInstanceAsync(createBindings, module);
        let phase = "parse";
        try {
          const deepCounts = Array.from(deep.parse(deepSource, deepName, 3));
          equal(deepCounts[2], 0, "deep left-addition parse diagnostics");
          phase = "parse-and-encode";
          const encoded = deep.parseAndEncode(deepSource, deepName, 3);
          assert(encoded.length > 0, "deep left-addition encoding");
          const deepHost = deep.createHost(bytes("/"));
          deepHost.addFile(deepName, deepSource, true);
          phase = "memory-host-compile";
          const deepSession = deepHost.compile({ noLib: true, target: 9, module: 99 });
          try {
            phase = "diagnostics";
            equal(deepSession.diagnostics().filter(value => value.file !== null), [], "deep left-addition source diagnostics");
            phase = "emit";
            const deepOutput = deepSession.emit();
            assert(!deepOutput.emit_skipped, "deep left-addition emits");
            equal(deepOutput.files.map(value => text(value.name)), ["/deep-left-addition.js"], "deep emitted filename");
            equal(text(deepOutput.files[0].text), `export const x = ${chain};\n`, "whole deep expression survives emit");
            return { fixture: "tools/s10/parser/fixtures.json#nested", nesting: 64, source_bytes: source.length, counts, emit_bytes: output.files[0].text.length,
              deep: { native_source_family: "crates/tsr_compiler/tests/t3_contracts.rs#deep_inputs_transform_through_the_growth_guards", terms, binary_depth: terms - 1, source_bytes: deepSource.length, counts: deepCounts, encoded_bytes: encoded.length, emit_bytes: deepOutput.files[0].text.length } };
          } finally { deepSession.dispose(); }
        } catch (error) { throw new Error(`deep left-addition failed during ${phase}: ${String(error)}\n${error.stack ?? ""}`); }
        finally { deep.dispose(); }
      } finally { session.dispose(); }
    } finally { instance.dispose(); }
  });

  await run("production-trap-invalidation-and-fresh-instance", async () => {
    const instance = await createInstanceAsync(createBindings, module);
    const held = instance.createHost(bytes("/"));
    const saved = instance.parseAndEncode(bytes("const n = 1;"), bytes("/before.ts"), 3);
    const expected = Array.from(saved);
    // This violates the existing public parser filename precondition:
    // AstBuilder::new_source_file requires a normalized absolute filename.
    const failure = throws(() => instance.parse(bytes("let x = 1;"), bytes("relative.ts"), 3), error => error instanceof WebAssembly.RuntimeError);
    equal(instance.state, "failed", "actual Rust abort poisons instance");
    throws(() => instance.parse(bytes("let x = 1;"), bytes("/valid.ts"), 3), error => String(error).includes("instance is failed"));
    throws(() => held.addDirectory(bytes("/later")), error => String(error).includes("disposed"));
    held.dispose();
    instance.dispose();
    equal(Array.from(saved), expected, "owned bytes survive actual trap");
    const fresh = await createInstanceAsync(createBindings, module);
    try {
      equal(fresh.parse(bytes("let x = 1;"), bytes("/valid.ts"), 3)[2], 0, "fresh production instance parses");
      return { trigger: "relative filename violates existing source-file precondition", failure: String(failure), poisoned_instance_refused: true, fresh_instance_passed: true };
    } finally { fresh.dispose(); }
  });

  result.state = result.cases.every(value => value.state === "passed") ? "passed" : "failed";
  return result;
}
