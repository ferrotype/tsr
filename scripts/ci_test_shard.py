#!/usr/bin/env python3
"""The `cargo test` target flags for one shard of a crate's integration tests.

    ci_test_shard.py <crate> <i>/<n>

Prints `--test a --test b ...` for the i-th of n alphabetical slices of the
crate's `tests/*.rs`; shard 1 also gets `--lib --bins --examples`. CI uses it
to split a crate whose test targets dominate the build (`tsr_compiler`: 59
integration targets, each linking the whole compiler) across runners without
a hand-kept list.
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def shard_flags(crate, index, count):
    tests = sorted(p.stem for p in (ROOT / "crates" / crate / "tests").glob("*.rs"))
    if not tests:
        sys.exit(f"{crate} has no integration tests under crates/{crate}/tests")
    if not 1 <= index <= count:
        sys.exit(f"shard {index}/{count}: the index must be between 1 and the shard count")
    flags = ["--lib", "--bins", "--examples"] if index == 1 else []
    for name in tests[index - 1::count]:
        flags += ["--test", name]
    return flags


def main(argv):
    if len(argv) != 3 or "/" not in argv[2]:
        sys.exit(__doc__)
    index, count = (int(part) for part in argv[2].split("/"))
    print(" ".join(shard_flags(argv[1], index, count)))


if __name__ == "__main__":
    main(sys.argv)
