#!/usr/bin/env python3
"""Record the JSX entities and alias links of the C4 cases in a pinned Go overlay.

The command line prints diagnostics only; the resolver's inputs are checker
state. This overlay adds two test files to the pinned checker package
(`export_test.go` reads `getJsxFactoryEntity`, `getJsxFragmentFactoryEntity`,
`getJsxNamespaceAt`, `getJsxNamespaceContainerForImplicitImport` and
`aliasSymbolLinks.referenced`; `oracle_test.go` builds each case's program from
its recorded command line) and records that state for every JSX and metadata
case of ../fixtures.json into native.json. Each row binds its case's flags and
source digests; provenance.json binds the pin, the request and the observer
sources. `upstream/` is never edited.
"""
import hashlib, json, pathlib, subprocess, sys
FIX = pathlib.Path(__file__).resolve().parent
CASES = FIX.parent
ROOT = pathlib.Path(__file__).resolve().parents[6]
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment, verified_upstream
UP = verified_upstream() / "tsc"
OUT = ROOT / "target/phase2/c4-state-native"
OUT.mkdir(parents=True, exist_ok=True)
sha = lambda b: hashlib.sha256(b).hexdigest()
# The command line the diagnostics recorder runs (../regenerate.py).
BASE = ["--noEmit", "--target", "esnext", "--ignoreConfig", "--pretty", "false", "--skipLibCheck"]
manifest = json.loads((CASES / "fixtures.json").read_text())
requests, bindings = [], {}
for case, spec in manifest.items():
    if not (spec["root"].endswith((".tsx", ".jsx")) or case.startswith("metadata_")):
        continue
    names = [spec["root"], *spec.get("files", [])]
    requests.append({"id": case, "root": spec["root"], "args": [*BASE, *spec["flags"], spec["root"]],
                     "files": {"/" + name: (CASES / name).read_text() for name in names}})
    bindings[case] = {"flags": spec["flags"], "sources_sha256": {name: sha((CASES / name).read_bytes()) for name in names}}
request_path = OUT / "requests.json"
request_path.write_text(json.dumps(requests, indent=1) + "\n")
env = go_environment()
observers = [FIX / "export_test.go", FIX / "oracle_test.go"]
subprocess.run(["gofmt", "-l", "-w", *map(str, observers)], env=env, check=True)
mapping = {str(UP / "internal/checker/c4_state_export_test.go"): str(FIX / "export_test.go"),
           str(UP / "internal/checker/c4_state_oracle_test.go"): str(FIX / "oracle_test.go")}
overlay = OUT / "overlay.json"
overlay.write_text(json.dumps({"Replace": mapping}))
raw = OUT / "native.json"
cmd = ["go", "test", "-mod=readonly", "-trimpath", "-overlay", str(overlay), "./internal/checker",
       "-run", "^TestC4State$", "-count=1", "-timeout=5m"]
env.update(C4_STATE_REQUESTS=str(request_path), C4_STATE_OUTPUT=str(raw))
subprocess.run(cmd, cwd=UP, env=env, check=True)
record = json.loads(raw.read_text())
for row in record["rows"]:
    row.update(bindings[row["id"]])
(FIX / "native.json").write_text(json.dumps(record, indent=1) + "\n")
receipt = {"pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
           "command": [part.replace(str(ROOT), "$ROOT") for part in cmd],
           "request_sha256": sha(request_path.read_bytes()), "output_sha256": sha((FIX / "native.json").read_bytes()),
           "observer_sources": {p.name: sha(p.read_bytes()) for p in [*observers, FIX / "regenerate.py"]},
           "replacements": {str(pathlib.Path(k).relative_to(UP)): sha(pathlib.Path(v).read_bytes()) for k, v in mapping.items()},
           "scope": "The pinned checker's JSX entities at each case's first JSX tag, checked and unchecked, "
                    "and aliasSymbolLinks.referenced of the root's import specifiers after checking."}
(FIX / "provenance.json").write_text(json.dumps(receipt, indent=1) + "\n")
print(f"{len(record['rows'])} cases recorded")
