#!/usr/bin/env python3
"""Freeze the emitter witnesses (docs/PHASE3-plan.md, T8; `tests/phase3_emitter.rs`).

    regenerate.py CAPTURE

CAPTURE is a native capture of exactly the rows below, taken with

    python3 scripts/phase3_native.py capture --texts --output CAPTURE --case ROW ...

Each row keeps its Rust request (the loading request S07 froze and the runner's
input groups, as `scripts/phase3_corpus.py` builds it, single-threaded) and the
pin's emit observation: `EmitSkipped`, `EmittedFiles`, the emit diagnostics and
the files the harness recorded, by name with their digests and text.

`EXACT` rows run through `CheckedProgram::emit`; `SEAM` rows also run through the
emitter with the script transformers the port has (see the test), because every
script chain of the pin includes a transformer the port does not have yet.
"""
import hashlib, json, pathlib, sys
FIX = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[6]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_corpus  # noqa: E402
import phase3_corpus  # noqa: E402
import phase3_inventory  # noqa: E402

EXACT = [f"compiler/{name}#configuration=0" for name in (
    "assertionWithNoArgument.ts", "compilerOptionsOutDirAndNoEmit.ts", "allowJsClassThisTypeCrash.ts",
    "isolatedModulesNoEmitOnError.ts", "noEmitOnError.ts", "DeclarationErrorsNoEmitOnError.ts",
    "exportAsNamespace.d.ts", "implicitAnyInAmbientDeclaration2.d.ts", "ambientClassDeclaredBeforeBase.ts",
    "accessorDeclarationEmitJs.ts", "jsFileCompilationEmitBlockedCorrectly.ts",
    "jsFileCompilationWithJsEmitPathSameAsInput.ts", "jsFileCompilationWithoutOut.ts",
    "filesEmittingIntoSameOutput.ts", "declarationFileOverwriteError.ts", "jsFileCompilationWithMapFileAsJs.ts",
    "jsFileCompilationWithMapFileAsJsWithInlineSourceMap.ts", "emitBOM.ts", "properties.ts", "2dArrays.ts")] + [
    "conformance/classes/propertyMemberDeclarations/autoAccessor1.ts#configuration=0"]
# The rows with outputs that need no unported transformer's change.
SEAM = [f"compiler/{name}#configuration=0" for name in (
    "jsFileCompilationEmitBlockedCorrectly.ts", "jsFileCompilationWithoutOut.ts",
    "jsFileCompilationWithMapFileAsJs.ts", "jsFileCompilationWithMapFileAsJsWithInlineSourceMap.ts")] + [f"compiler/{name}.ts#configuration=0" for name in (
    "inlineSourceMap", "inlineSourceMap2", "optionsInlineSourceMapMapRoot", "optionsInlineSourceMapSourceRoot",
    "optionsInlineSourceMapSourcemap", "optionsSourcemapInlineSources", "optionsSourcemapInlineSourcesMapRoot",
    "optionsSourcemapInlineSourcesSourceRoot", "sourceMap-Comment1", "sourceMap-EmptyFile1", "sourceMap-LineBreaks",
    "sourceMap-NewLine1", "sourceMap-SemiColon1", "sourceMapUnclosedBlock",
    "sourceMapValidationVarInDownLevelGenerator", "removeComments", "commentOnBinaryOperator2",
    "commentOnIfStatement1", "commonSourceDir1", "commonSourceDir3", "constDeclarations-useBeforeDefinition2",
    "newLineFlagWithLF", "bom-utf8", "bom-utf16le", "acceptSymbolAsWeakType", "bigintIndex",
    "negatedUnicodeSetUnionMayContainStrings", "ArrowFunctionExpression1", "alwaysStrictES6",
    "sourceMap-InterfacePrecedingVariableDeclaration1")] + [
    "compiler/indexAt.ts#configuration=1", "compiler/indexAt.ts#configuration=0"] + [
    f"compiler/numericUnderscoredSeparator.ts#configuration={index}" for index in (1, 2, 3, 4)]

capture = pathlib.Path(sys.argv[1]).resolve()
report = json.loads((capture / "report.json").read_text())
raw = (capture / "observations.ndjson").read_bytes()
if hashlib.sha256(raw).hexdigest() != report["observation_sha256"] or not report["texts"]:
    sys.exit("the capture changed since its report, or it was taken without --texts")
observed = {row["id"]: row for row in map(json.loads, raw.splitlines())}
if sorted(observed) != sorted(set(EXACT + SEAM)):
    sys.exit("the capture holds other rows than the witness")
inventory = {row["id"]: row for row in phase3_inventory.read()["rows"]}
loading = phase2_corpus.loading_requests()
rows = []
for row_id in dict.fromkeys(EXACT + SEAM):
    native = observed[row_id]
    if native["state"] != "executed" or native["emit"]["state"] != "executed":
        sys.exit("the pin did not emit " + row_id)
    request = phase3_corpus.build_request(inventory[row_id], native, loading[row_id], "single")
    request = {key: request[key] for key in ("id", "acceptance_tier", "loading", "mode", "error_inputs")}
    emit = native["emit"]
    rows.append({"id": row_id, "seam": row_id in SEAM, "request": request,
                 "emit_skipped": emit["emit_skipped"], "emitted_files_hex": emit["emitted_files_hex"],
                 "diagnostics": [(item["code"], item["args_hex"]) for item in emit["diagnostics"]],
                 "source_maps": emit["source_maps"], "outputs": native["outputs"]})
record = {"pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
          "capture": {key: report[key] for key in ("observation_sha256", "request_sha256", "mode")},
          "rows": rows}
(FIX / "emitter.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
print(json.dumps({"rows": len(rows), "seam": len(SEAM), "exact": len(EXACT)}))
