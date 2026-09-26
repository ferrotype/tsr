#!/usr/bin/env python3
"""Capture public checker observations in an isolated native test overlay."""
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[6]
FIX = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment, verified_upstream

up = verified_upstream() / "tsc"
out = ROOT / "target/phase2/c2-js-full-signature-native"
out.mkdir(parents=True, exist_ok=True)
env = go_environment()
subprocess.run(["gofmt", "-w", str(FIX / "oracle_test.go")], env=env, check=True)
mapping = {str(up / "internal/checker/c2_js_full_signature_test.go"): str(FIX / "oracle_test.go")}
overlay = out / "overlay.json"
overlay.write_text(json.dumps({"Replace": mapping}))
cmd = ["go", "test", "-mod=readonly", "-trimpath", "-overlay", str(overlay),
       "./internal/checker", "-run", "^TestC2JSFullSignatures$", "-count=1", "-timeout=3m"]
env.update(C2_REQUESTS=str(FIX / "requests.json"), C2_OUTPUT=str(FIX / "native.json"))
subprocess.run(cmd, cwd=up, env=env, check=True)
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
(FIX / "provenance.json").write_text(json.dumps({
    "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
    "command": cmd,
    "request_sha256": sha(FIX / "requests.json"),
    "output_sha256": sha(FIX / "native.json"),
    "observer_sources": {p: sha(FIX / p) for p in ["oracle_test.go", "regenerate.py"]},
    "replacements": {str(pathlib.Path(k).relative_to(up)): sha(pathlib.Path(v)) for k, v in mapping.items()},
    "scope": "Public diagnostics and declaration-name type queries; ES2015/CommonJS, allowJs/checkJs, noEmit, skipDefaultLibCheck. Four original corpus source sets and two narrowed alias schedules. No production Go changes."
}, indent=2) + "\n")
