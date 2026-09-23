"""Reproduce the pinned private JSX navigation observations, without editing the pin."""
import argparse
import json
from pathlib import Path
import shutil

import phase1_capture as capture
from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path("tools/phase1/syntax/astnav-rescan")


def tsv(requests, observed):
    rows = observed["observations"]
    if [r["case"] for r in requests["requests"]] != [r["case"] for r in rows]:
        raise ValueError("navigation observations differ from request schedule")
    lines = ["# input hex\tjsx-child\tbefore\tafter\tstart\tend\tflags"]
    for request, row in zip(requests["requests"], rows):
        lines.append("\t".join([request["Text"].encode().hex(), str(request["JSX"]).lower(),
                                *[str(row[key]) for key in ("before", "after", "start", "end", "flags")]]))
    return "\n".join(lines) + "\n"


def observe(output, write=False):
    source = ROOT / FIXTURE
    requests = strict_json_loads((source / "requests.json").read_bytes())
    observed = capture.run_probe(output, "astnav", (source / "probe_test.go").read_text(),
                                 requests, "TestPhase1NavigationRescan", False)
    rendered = tsv(requests, observed)
    (output / "navigation-rescan.tsv").write_text(rendered)
    recorded = strict_json_loads((source / "native-observations.json").read_bytes())
    equal = recorded["observations"] == observed["observations"]
    if write:
        shutil.copyfile(output / "observations.json", source / "native-observations.json")
        shutil.copyfile(output / "provenance.json", source / "native-provenance.json")
        (ROOT / "crates/tsr_astnav/src/testdata/navigation-rescan.tsv").write_text(rendered)
    return {"match": equal, "cases": len(observed["observations"]), "output": str(output)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new, non-existing output directory")
    parser.add_argument("--write", action="store_true", help="replace frozen native observations and derived TSV")
    args = parser.parse_args()
    result = observe(args.output.resolve(), args.write)
    print(json.dumps(result, sort_keys=True))
    if not result["match"] and not args.write:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
