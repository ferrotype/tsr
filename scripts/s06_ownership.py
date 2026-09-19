"""Measured S06 AST-storage coverage inside the shared E3 ownership capture.

The S04 report's seven counter-scenario rows remain unchanged. This additional
inventory names the real AST tests that must all execute in each of four modes;
its per-mode metrics and retained stderr never imply full future E3 coverage.
"""

from pathlib import Path
import re
import sys

from s04_common import strict_json_loads

CASE_MANIFEST = Path("data/s06/ownership-cases.json")


def load_cases(root):
    cases = strict_json_loads((Path(root) / CASE_MANIFEST).read_bytes())
    if (not isinstance(cases, list) or not cases
            or any(not isinstance(name, str) or re.fullmatch(
                r"storage_tests::[a-z0-9_]+", name) is None for name in cases)
            or cases != sorted(set(cases))):
        raise ValueError("AST ownership inventory must be nonempty, sorted, unique exact test names")
    return cases


def validate_output(output, cases, mode, scope="AST ownership"):
    """Check observations, not Cargo's zero exit status or an aggregate count."""
    text = output.decode("utf-8")
    sys.stderr.write(text)
    rows = re.findall(r"^test (\S+) \.\.\. (\S+)$", text, re.MULTILINE)
    actual = [name for name, _ in rows]
    missing = sorted(set(cases) - set(actual))
    extra = sorted(set(actual) - set(cases))
    duplicate = sorted({name for name in actual if actual.count(name) != 1})
    failed = [f"{name}: {status}" for name, status in rows if status != "ok"]
    summaries = re.findall(
        r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"(\d+) measured; (\d+) filtered out;.*$", text, re.MULTILINE)
    running = re.findall(r"^running (\d+) tests?$", text, re.MULTILINE)
    summary_valid = (len(summaries) == 1 and summaries[0][0] == "ok"
                     and int(summaries[0][1]) == len(cases)
                     and all(int(value) == 0 for value in summaries[0][2:5]))
    if (missing or extra or duplicate or failed or not summary_valid
            or running != [str(len(cases))] or len(rows) != len(cases)):
        details = []
        for label, values in (("missing", missing), ("unexpected", extra),
                              ("duplicate", duplicate), ("not passed", failed)):
            if values:
                details.append(f"{label}: {', '.join(values)}")
        if not summary_valid or running != [str(len(cases))]:
            details.append(f"expected one complete {len(cases)}-test summary with no ignores")
        raise ValueError(f"{scope} {mode}: " + "; ".join(details))


def measure(root, invoke, prefix, options, env, cases, mode):
    command = [*prefix, "test", "--package", "tsr_ast", "--lib", "--locked",
               *options, "storage_tests::", "--", "--test-threads=1", "--nocapture"]
    try:
        validate_output(invoke(root, command, env), cases, mode)
    except (RuntimeError, ValueError) as error:
        print(f"AST ownership {mode} failed: {error}", file=sys.stderr)
        return False
    return True
