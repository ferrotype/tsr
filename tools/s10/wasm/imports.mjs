// Exact wasm-bindgen ABI imports for the pinned toolchain. Any new capability
// requires review; accepting a module name alone would permit host I/O hooks.
export function checkImports(module, mode) {
  const imports = WebAssembly.Module.imports(module);
  const names = new Set([
    "__wbindgen_init_externref_table",
    "__wbindgen_cast_0000000000000001",
  ]);
  // Only the public checker exports owned JS classes and their throw shim.
  // The private corpus harness exports only observe_corpus.
  if (mode === "checker") names.add("__wbg___wbindgen_throw_344f42d3211c4765");
  if (imports.length !== names.size || !imports.every(value =>
    value.module === `./${mode}_bg.js` && value.kind === "function" && names.delete(value.name))) {
    throw new Error(`unreviewed wasm imports: ${JSON.stringify(imports)}`);
  }
  return imports;
}
