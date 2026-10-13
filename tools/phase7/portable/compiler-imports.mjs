// Independent ABI policy for the pinned test-only compiler consumer. Build
// metadata records what was built; it cannot authorize new host capabilities.
export function checkCompilerImports(module) {
  const imports = WebAssembly.Module.imports(module);
  const names = new Set([
    "__wbg_tsrPortableCompilerCallback_fedf4f42051b2f81",
    "__wbg___wbindgen_throw_344f42d3211c4765",
    "__wbg___wbindgen_debug_string_c25d447a39f5578f",
    "__wbindgen_init_externref_table",
  ]);
  if (imports.length !== names.size || !imports.every(value =>
    value.module === "./compiler_bg.js" && value.kind === "function" && names.delete(value.name))) {
    throw new Error(`unreviewed compiler wasm imports: ${JSON.stringify(imports)}`);
  }
  return imports;
}
