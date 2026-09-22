#!/usr/bin/env python3
"""The existing S03 producer plus independently regenerated F1b locale tables.

Keep the original measurements unchanged. locale_complete is measured only by
successful exact regeneration of both locale outputs, never by file existence.
"""
import json
from pathlib import Path
import subprocess
import sys

from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
LOCALE_OUTPUTS = {
    "crates/tsr_locale/src/tables_generated.rs",
    "crates/tsr_locale/tests/native_generated.rs",
}


def capture(root=ROOT, run=subprocess.run):
    generated = run(["cargo", "xtask", "gen", "--verify"], cwd=root,
                    stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
    result = strict_json_loads(generated.stdout)
    if not isinstance(result, dict) or not isinstance(result.get("metrics"), dict):
        raise ValueError("generation producer did not emit metrics")
    if "locale_complete" in result["metrics"]:
        raise ValueError("locale_complete already has another producer")
    locale = run([sys.executable, "scripts/generate_locale_tables.py", "--check"],
                 cwd=root, stdout=sys.stderr, stderr=sys.stderr, check=False)
    manifest = strict_json_loads((root / "data/phase1/locale-tables-manifest.json").read_bytes())
    # Missing generation outputs are a failed measured criterion, not success
    # borrowed from the existing AST/diagnostic/API generator.
    result["metrics"]["locale_complete"] = (
        locale.returncode == 0 and set(manifest.get("outputs", {})) == LOCALE_OUTPUTS
    )
    return result


if __name__ == "__main__":
    try:
        print(json.dumps(capture(), sort_keys=True))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Phase 1 generation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
