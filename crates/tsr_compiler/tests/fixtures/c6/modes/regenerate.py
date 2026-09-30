#!/usr/bin/env python3
"""Freeze the C6 two-mode witness (docs/PHASE2-C6-plan.md, contract 8).

One executed corpus row with several files, cross-file types and unions to
order: its Rust request, built by `phase2_corpus.requests` without a mode (the
contract sets each mode), and the native observations of both test-program
modes, taken from the verified single-threaded and concurrent captures. The
native errors, types, symbols and public type strings agree across the modes;
union ordering's checker and union counts are where the pin's modes differ.
"""
import hashlib, json, pathlib, sys
FIX = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[6]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_corpus  # noqa: E402

ROW = "conformance/dynamicImport/importCallExpression4ES2020.ts#configuration=0"
CAPTURES = {"single": ROOT / "target/phase2/native", "concurrent": ROOT / "target/phase2/native-concurrent"}
FIELDS = ("errors", "types", "symbols", "public_type_strings", "union_ordering")

_, requests, _ = phase2_corpus.requests(CAPTURES["single"], cases=[ROW], mode=None)
(request,) = requests
raw = json.dumps(request, indent=1, sort_keys=True) + "\n"
(FIX / "request.json").write_text(raw)
modes, captures = {}, {}
for mode, directory in CAPTURES.items():
    _, report, rows = phase2_corpus.load_native(directory, mode)
    if not (directory / "verified.json").exists():
        sys.exit(f"the {mode} capture is not verified")
    (row,) = [row for row in rows if row["id"] == ROW]
    modes[mode] = {field: row[field] for field in FIELDS}
    captures[mode] = report["observation_sha256"]
if any(modes["single"][field] != modes["concurrent"][field] for field in FIELDS[:-1]):
    sys.exit("the witness's native modes differ outside union ordering")
if modes["single"]["union_ordering"] == modes["concurrent"]["union_ordering"]:
    sys.exit("the witness's native modes do not differ")
record = {"pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"], "id": ROW,
          "request_sha256": hashlib.sha256(raw.encode()).hexdigest(), "captures": captures, "modes": modes}
(FIX / "native.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
print(json.dumps({mode: modes[mode]["union_ordering"] for mode in modes}))
