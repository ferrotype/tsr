"""Temporary native reproduction of the CI SIGSEGV; never parity evidence."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crash-output"
RUNNER = ROOT / "runner/tsr-testrunner"
VARIANT = "conformance/typeOfThisInStaticMembers11(target=esnext).ts"


def main():
    OUT.mkdir(exist_ok=True)
    deadline = time.monotonic() + 300
    stopped = threading.Event()

    def worker(index):
        command = [str(RUNNER), "--suite", "compiler", "--mode", "concurrent",
                   "run", "--id", VARIANT, "--local", str(OUT / f"local-{index}")]
        count = 0
        while time.monotonic() < deadline and not stopped.is_set():
            result = subprocess.run(command, cwd=ROOT, capture_output=True, timeout=30)
            count += 1
            if result.returncode:
                stopped.set()
                prefix = OUT / f"failure-{index}-{count}"
                prefix.with_suffix(".stdout").write_bytes(result.stdout)
                prefix.with_suffix(".stderr").write_bytes(result.stderr)
                prefix.with_suffix(".json").write_text(json.dumps({
                    "command": command, "returncode": result.returncode,
                    "worker": index, "iteration": count,
                }, indent=2) + "\n")
                print(f"worker {index}: exit {result.returncode} after {count} runs", flush=True)
                break
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            if len(rows) != 9 or any(row["state"] != "pass" for row in rows):
                raise RuntimeError(f"unexpected result: {result.stdout!r}")
        return count

    with ThreadPoolExecutor(max_workers=4) as pool:
        counts = list(pool.map(worker, range(4)))
    report = {"runs": sum(counts), "per_worker": counts, "crashed": stopped.is_set(),
              "cpus": os.cpu_count(), "variant": VARIANT}
    (OUT / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
