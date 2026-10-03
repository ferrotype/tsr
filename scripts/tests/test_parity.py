"""scripts/parity.py over a fake runner: sharding, crash and deadline handling,
check against the expectation file, accept keeping reasons and approvals."""
from __future__ import annotations

import json
import os
from pathlib import Path
import stat
import sys

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import parity  # noqa: E402

FAKE_RUNNER = r'''#!/usr/bin/env python3
import json, os, sys, time
args = sys.argv[1:]
command = args[args.index("list")] if "list" in args else "run"
VARIANTS = ["compiler/a.ts", "compiler/b(target=es2015).ts", "compiler/crash.ts", "compiler/slow.ts",
            "conformance/c.ts"]
if command == "list":
    print("\n".join(VARIANTS))
    sys.exit(0)
variant = args[args.index("--id") + 1]
if variant == "compiler/crash.ts":
    print(json.dumps({"id": variant + "/error", "state": "pass"}))
    print("boom", file=sys.stderr)
    sys.exit(101)
if variant == "compiler/slow.ts":
    time.sleep(5)
for subtest in ("error", "types", "symbols"):
    state = "fail" if (variant, subtest) in {("compiler/a.ts", "types"), ("conformance/c.ts", "error")} else "pass"
    row = {"id": f"{variant}/{subtest}", "state": state}
    if state == "fail":
        row["reason"] = "baseline differs"
        row["detail"] = "line 1\nline 2"
    if os.environ.get("FAKE_FIXED") and variant == "compiler/a.ts":
        row["state"] = "pass"
    print(json.dumps(row))
if variant == "compiler/b(target=es2015).ts":
    print(json.dumps({"id": variant + "/output", "state": "skip", "reason": "nondeterministic"}))
'''


@pytest.fixture
def fake(tmp_path, monkeypatch):
    runner = tmp_path / "runner.py"
    runner.write_text(FAKE_RUNNER)
    runner.chmod(runner.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setattr(parity, "PARITY", tmp_path / "parity")
    monkeypatch.setattr(parity, "pin", lambda: "deadbeef")
    monkeypatch.setitem(parity.SUITES, "compiler", parity.Suite("x", "x", ("--suite", "compiler"), 1))
    return runner


def run(fake, output, *extra):
    parity.main(["run", "compiler", "--runner", str(fake), "--output", str(output), "--jobs", "2", *extra])
    return [json.loads(line) for line in (output / "results.ndjson").read_text().splitlines()]


def test_run_records_every_subtest_and_fails_crashes_and_deadlines_as_the_variant(fake, tmp_path, capsys):
    rows = run(fake, tmp_path / "out", "--timeout", "1")
    by_id = {row["id"]: row for row in rows}
    assert by_id["compiler/a.ts/types"]["state"] == "fail"
    assert by_id["compiler/b(target=es2015).ts/output"]["state"] == "skip"
    assert by_id["compiler/crash.ts"] == {"id": "compiler/crash.ts", "state": "fail", "reason": "exit 101", "detail": "boom"}
    assert by_id["compiler/slow.ts"]["reason"] == "deadline: 1 s"
    meta = json.loads((tmp_path / "out/meta.json").read_text())
    assert meta["total"] == 5 and meta["selected"] == 5 and meta["shard"] == [1, 1]
    assert meta["counts"]["fail"] == 4 and meta["partial"] is False
    assert "compiler/slow.ts" in {entry["variant"] for entry in meta["slowest"]}


def variants_of(rows):
    return {row["id"].rsplit("/", 1)[0] if row["id"].count("/") > 1 else row["id"] for row in rows}


def test_shards_partition_the_sorted_variants(fake, tmp_path):
    first = run(fake, tmp_path / "one", "--shard", "1/2", "--timeout", "1")
    second = run(fake, tmp_path / "two", "--shard", "2/2", "--timeout", "1")
    assert variants_of(first) == {"compiler/a.ts", "compiler/crash.ts", "conformance/c.ts"}
    assert variants_of(second) == {"compiler/b(target=es2015).ts", "compiler/slow.ts"}


def test_check_requires_every_shard_once_and_refuses_partial_runs(fake, tmp_path):
    run(fake, tmp_path / "one", "--shard", "1/2", "--timeout", "1")
    with pytest.raises(SystemExit, match="every shard"):
        parity.main(["check", "compiler", "--results", str(tmp_path / "one")])
    run(fake, tmp_path / "dev", "--id", "compiler/a.ts", "--timeout", "1")
    with pytest.raises(SystemExit, match="development check"):
        parity.main(["accept", "compiler", "--results", str(tmp_path / "dev")])


def test_accept_then_check_passes_and_keeps_reasons_and_approvals(fake, tmp_path, capsys, monkeypatch):
    run(fake, tmp_path / "one", "--shard", "1/2", "--timeout", "1")
    run(fake, tmp_path / "two", "--shard", "2/2", "--timeout", "1")
    results = ["--results", str(tmp_path / "one"), str(tmp_path / "two")]
    parity.main(["accept", "compiler", *results])
    document = json.loads((tmp_path / "parity/compiler.json").read_text())
    assert document["pin"] == "deadbeef" and document["total"] == 5
    assert list(document["failing"]) == sorted(document["failing"])
    assert document["failing"]["compiler/a.ts/types"] == {"reason": "baseline differs", "detail": "line 1"}
    assert document["failing"]["compiler/crash.ts"]["reason"] == "exit 101"
    # The owner annotates an entry; accept keeps the annotation.
    document["failing"]["conformance/c.ts/error"] = {"reason": "known TS6059 ordering", "approved": "owner 2026-10-03"}
    (tmp_path / "parity/compiler.json").write_text(json.dumps(document))
    parity.main(["check", "compiler", *results])
    out = capsys.readouterr().out
    assert "fail 4 (1 approved)" in out and "new failure" not in out
    parity.main(["accept", "compiler", *results])
    document = json.loads((tmp_path / "parity/compiler.json").read_text())
    assert document["failing"]["conformance/c.ts/error"]["approved"] == "owner 2026-10-03"
    # A test that starts passing fails the check until accepted; a new failure fails it too.
    monkeypatch.setenv("FAKE_FIXED", "1")
    run(fake, tmp_path / "one", "--shard", "1/2", "--timeout", "1")
    with pytest.raises(SystemExit):
        parity.main(["check", "compiler", *results])
    out = capsys.readouterr().out
    assert "1 named failure(s) now pass" in out and "compiler/a.ts/types" in out
    parity.main(["accept", "compiler", *results])
    document = json.loads((tmp_path / "parity/compiler.json").read_text())
    assert "compiler/a.ts/types" not in document["failing"]


def test_check_reports_pin_and_denominator_drift(fake, tmp_path, capsys):
    run(fake, tmp_path / "out", "--timeout", "1")
    parity.main(["accept", "compiler", "--results", str(tmp_path / "out")])
    document = json.loads((tmp_path / "parity/compiler.json").read_text())
    document["pin"], document["total"] = "cafe", 4
    (tmp_path / "parity/compiler.json").write_text(json.dumps(document))
    with pytest.raises(SystemExit):
        parity.main(["check", "compiler", "--results", str(tmp_path / "out")])
    out = capsys.readouterr().out
    assert "pin deadbeef differs" in out and "5 variants enumerated; the expectation file records 4" in out


def test_step_summary_is_appended_when_github_sets_it(fake, tmp_path, monkeypatch):
    run(fake, tmp_path / "out", "--timeout", "1")
    parity.main(["accept", "compiler", "--results", str(tmp_path / "out")])
    summary = tmp_path / "summary.md"
    monkeypatch.setenv("GITHUB_STEP_SUMMARY", str(summary))
    parity.main(["check", "compiler", "--results", str(tmp_path / "out")])
    assert summary.read_text().startswith("### parity: compiler")
