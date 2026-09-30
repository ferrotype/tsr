#!/usr/bin/env python3
"""Freeze the C7 content-mapper witnesses (docs/PHASE2-C7-plan.md, C7.8.5).

The 15 executed rows that run content mappers: each row's Rust request, built
by `phase2_corpus.requests` without a mode (the contracts set each mode), and
the native error observations of both test-program modes, taken from the
verified single-threaded and concurrent captures. The contracts also compare
each row's content-mapper baseline with the pin's committed reference, which
they read from the upstream checkout.
"""
import hashlib, json, pathlib, sys
FIX = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_corpus  # noqa: E402

NAMES = ("AliasDiagnostic", "DeclarationEmit", "DeclarationEmitFailure", "DiagnosticDirectives", "Diagnostics",
         "InvalidDiagnosticDirectives", "InvalidExtension", "PerFileExtension", "SupplementalDiagnostics",
         "SupplementalFileCollision", "SupplementalGlobals", "SupplementalModule", "Symlink",
         "SynthesizedUnusedDiagnostics", "Transform")
ROWS = [f"compiler/contentMapper{name}.ts#configuration=0" for name in NAMES]
CAPTURES = {"single": ROOT / "target/phase2/native", "concurrent": ROOT / "target/phase2/native-concurrent"}
FIELDS = ("error_pre_diagnostics", "error_post_diagnostics", "error_diagnostics", "error_render_inputs",
          "error_pretty", "errors")

_, requests, _ = phase2_corpus.requests(CAPTURES["single"], cases=ROWS, mode=None)
if sorted(request["id"] for request in requests) != sorted(ROWS):
    sys.exit("the frozen inventory no longer executes every content-mapper row")
raw = json.dumps(sorted(requests, key=lambda request: request["id"]), indent=1, sort_keys=True) + "\n"
(FIX / "requests.json").write_text(raw)
rows, captures = {row: {} for row in ROWS}, {}
for mode, directory in CAPTURES.items():
    _, report, observed = phase2_corpus.load_native(directory, mode)
    if not (directory / "verified.json").exists():
        sys.exit(f"the {mode} capture is not verified")
    by_id = {row["id"]: row for row in observed}
    for row in ROWS:
        rows[row][mode] = {field: by_id[row][field] for field in FIELDS}
    captures[mode] = report["observation_sha256"]
record = {"pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
          "requests_sha256": hashlib.sha256(raw.encode()).hexdigest(), "captures": captures, "rows": rows}
(FIX / "native.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
print(json.dumps({row.split("/")[1]: len(rows[row]["single"]["error_post_diagnostics"]) for row in ROWS}))
