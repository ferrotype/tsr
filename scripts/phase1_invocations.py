"""Map each config/options reference output to the native test that renders it.

Runs the pinned `tsoptions` test package with an access-only instrumentation
patch inserted into `internal/testutil/baseline/baseline.go`. The patch records
which test called `baseline.Run`, for which output, with what content digest.

Two properties come out of the same run:

* **Invocation mapping.** Each output is attributed to the exact Go test (and
  subtest) that produced it, rather than to a generic renderer name.
* **Verified rendering.** `baseline.Run` still calls the pinned
  `writeComparison`, so the run only passes when the rendered content
  reproduces the committed reference bytes. A recorded digest that equals the
  frozen file's digest is therefore an observation, not an assumption.

Nothing here reads an expected result to fill in a gap, and nothing compares
Rust. `config/matchFiles` produces no rows, which is the point: no pinned test
renders it.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import command  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

BASELINE_SOURCE = "tsc/internal/testutil/baseline/baseline.go"
FRAGMENT = ROOT / "tools/phase1/config/baseline_recorder.go.fragment"
PACKAGE = "tsoptions"

# The exact pinned text the recorder hooks into. Asserting it keeps the patch
# honest: if the pin moves and this body changes, the patch fails loudly rather
# than silently recording nothing.
ANCHOR = """func Run(t *testing.T, fileName string, actual string, opts Options) {
	subfolder := opts.Subfolder
	localPath := filepath.Join(localRoot, subfolder, fileName)
	referencePath := filepath.Join(referenceRoot, subfolder, fileName)
	recordBaseline(t, filepath.Join(subfolder, fileName))
	writeComparison(t, actual, localPath, referencePath)
}"""

REPLACEMENT = """func Run(t *testing.T, fileName string, actual string, opts Options) {
	subfolder := opts.Subfolder
	localPath := filepath.Join(localRoot, subfolder, fileName)
	referencePath := filepath.Join(referenceRoot, subfolder, fileName)
	phase1RecordInvocation(t, subfolder, fileName, actual)
	recordBaseline(t, filepath.Join(subfolder, fileName))
	writeComparison(t, actual, localPath, referencePath)
}"""

REQUIRED_IMPORTS = (
    ("crypto/sha256", '"crypto/sha256"'),
    ("encoding/hex", '"encoding/hex"'),
    ("encoding/json", '"encoding/json"'),
    ("os", '"os"'),
    ("sync", '"sync"'),
)


def instrumented_source() -> tuple[str, str]:
    """The pinned baseline.go with the recorder inserted. Returns (text, digest)."""
    upstream = verified_upstream()
    original = (upstream / BASELINE_SOURCE).read_text()
    if ANCHOR not in original:
        raise ValueError(
            f"{BASELINE_SOURCE} no longer contains the anchored Run body at this pin; "
            "review the instrumentation patch before recording invocations"
        )
    patched = original.replace(ANCHOR, REPLACEMENT, 1)

    # Add only the imports the fragment needs and the file does not already have.
    start = patched.index("import (")
    end = patched.index(")", start)
    block = patched[start:end]
    additions = [spec for name, spec in REQUIRED_IMPORTS if f'"{name}"' not in block]
    if additions:
        patched = patched[:start + len("import (")] + "\n\t" + "\n\t".join(additions) + patched[start + len("import ("):]

    patched += "\n" + FRAGMENT.read_text().split("// scripts/phase1_invocations.py asserts the anchor it replaces is present in\n// the pinned source, so this fragment cannot silently stop applying.\n", 1)[1]
    return patched, hashlib.sha256(patched.encode()).hexdigest()


def record(directory: Path) -> list[dict]:
    """Run the pinned tsoptions tests and return the recorded invocations."""
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()

    patched, patched_digest = instrumented_source()
    source_path = directory / "baseline.go"
    source_path.write_text(patched)
    overlay = directory / "overlay.json"
    overlay.write_bytes(
        canonical({"Replace": {str(upstream / BASELINE_SOURCE): str(source_path)}})
    )
    recording = directory / "invocations.ndjson"
    env["PHASE1_BASELINE_RECORD"] = str(recording)

    # No -trimpath: the tsoptions test package links internal/testutil/baseline,
    # whose init resolves the repository root through runtime.Caller.
    stdout = command(
        ["go", "test", "-mod=readonly", "-overlay", str(overlay), f"./internal/{PACKAGE}",
         "-count=1", "-timeout=15m"],
        cwd=upstream / "tsc",
        env=env,
    )
    (directory / "go-test.stdout").write_bytes(stdout)
    verified_upstream()

    rows = []
    if recording.exists():
        for line in recording.read_text().splitlines():
            if line.strip():
                rows.append(json.loads(line))
    (directory / "provenance.json").write_bytes(
        canonical(
            {
                "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
                "package": PACKAGE,
                "instrumented_source_sha256": patched_digest,
                "fragment_sha256": hashlib.sha256(FRAGMENT.read_bytes()).hexdigest(),
                "recorded_invocations": len(rows),
                "go_test_passed": True,
                "toolchain_local": env["GOTOOLCHAIN"] == "local",
            }
        )
        + b"\n"
    )
    return rows


def attribute(rows: list[dict], index: dict) -> dict:
    """Attach per-output invocation and rendering verification to the index.

    An output is `verified` only when a pinned test rendered it during this run
    and the digest it rendered equals the committed reference file's digest.
    """
    upstream = verified_upstream()
    by_path: dict[str, dict] = {}
    for row in rows:
        by_path.setdefault(f"{row['subfolder']}/{row['file']}", []).append(row)

    summary = {"verified": 0, "unrendered": 0, "mismatched": 0, "unexpected": []}
    indexed_paths = set()
    for group, value in index["groups"].items():
        subdirectory = value["subdirectory"].removeprefix("tsc/testdata/baselines/reference/")
        for output in value["outputs"]:
            key = f"{subdirectory}/{output['name']}"
            indexed_paths.add(key)
            recorded = by_path.get(key, [])
            if not recorded:
                output["invocation"] = None
                output["rendering_verified"] = False
                output["rendering_status"] = "no pinned test rendered this output"
                summary["unrendered"] += 1
                continue
            if len(recorded) > 1:
                names = sorted({r["test"] for r in recorded})
                output["invocation"] = {"tests": names, "renders": len(recorded)}
                output["rendering_verified"] = False
                output["rendering_status"] = "several invocations wrote the same output"
                summary["mismatched"] += 1
                continue
            row = recorded[0]
            matches = row["sha256"] == output["sha256"]
            output["invocation"] = {"test": row["test"], "rendered_bytes": row["bytes"]}
            output["rendering_verified"] = matches
            output["rendering_status"] = (
                "the pinned test rendered exactly the committed reference bytes"
                if matches
                else "the pinned test rendered different bytes than the committed reference"
            )
            summary["verified" if matches else "mismatched"] += 1

    summary["unexpected"] = sorted(set(by_path) - indexed_paths)
    index["invocation_mapping"] = {
        "recorded_invocations": len(rows),
        "verified_outputs": summary["verified"],
        "unrendered_outputs": summary["unrendered"],
        "problem_outputs": summary["mismatched"],
        "outputs_rendered_but_not_indexed": summary["unexpected"],
    }
    for group, value in index["groups"].items():
        outputs = value["outputs"]
        verified = sum(1 for o in outputs if o.get("rendering_verified"))
        value["authority"]["verified_outputs"] = verified
        value["authority"]["mapping_complete"] = verified == len(outputs)
    return index
