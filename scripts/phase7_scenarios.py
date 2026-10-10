#!/usr/bin/env python3
"""The five Phase 7 benchmarking scenarios (docs/PHASE7-plan.md, R0 item 1).

    phase7_scenarios.py provision [NAME...] [--clean]
    phase7_scenarios.py freeze [NAME...]
    phase7_scenarios.py verify [NAME...]

`provision` downloads each scenario's commit archive and installs its
dependencies in the descriptor's pinned container, under
<workloads>/phase7.noindex/<name>-<commit12> (the `.noindex` suffix keeps
Spotlight out of the dependency trees). `freeze` runs the oracle, tsgo built
from the pin (tools/phase7/bench/capture.py build), in both modes at four
checkers and writes the descriptor's `work`. `verify` reruns the oracle and
fails on any difference from the frozen work: run after `provision --clean`,
it is the witness that the descriptors reproduce their digests from a clean
cache. Descriptors and their fields: tools/phase7/bench/common.py.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.phase7.bench import common  # noqa: E402


def oracle_work(descriptor, oracle):
    """The oracle's facts for both modes, as the descriptor records them."""
    root = common.checkout(descriptor)
    if not common.provisioned(descriptor):
        raise ValueError(f"{descriptor['name']}: provision it first")
    common.require_unchanged(descriptor)
    work = {}
    for mode in common.MODES:
        runs = []
        # Emit runs repeat. A descriptor that declares its oracle's emit
        # varying takes the larger count, so the union covers what the runs
        # drop; elsewhere a variation stops the freeze for investigation.
        varies = bool(descriptor.get('oracle_emit_varies'))
        wanted = (common.ORACLE_EMIT_RUNS[1] if varies else common.ORACLE_EMIT_RUNS[0]) if mode == 'emit' else 1
        while len(runs) < wanted:
            out = common.emit_directory() if mode == 'emit' else None
            try:
                result = common.run(oracle, root, common.arguments(descriptor, mode, common.ORACLE_CHECKERS, out))
                runs.append(common.observe(result, root, out))
            finally:
                if out is not None:
                    shutil.rmtree(out, ignore_errors=True)
        facts = runs[0]
        for other in runs[1:]:
            for field in ('exit', 'output_sha256', 'files', 'listed', 'diagnostics'):
                if other[field] != facts[field]:
                    raise ValueError(f"{descriptor['name']} {mode}: the oracle's {field} varies between runs")
        options, _ = common.options_digest(oracle, descriptor, mode, root)
        work[mode] = {'checkers': common.ORACLE_CHECKERS, 'exit': facts['exit'], 'files': facts['files'],
                      'listed': facts['listed'], 'inputs_sha256': common.inputs_digest(root, facts['listed_paths']),
                      'options_sha256': options, 'output_sha256': facts['output_sha256'],
                      'diagnostics': facts['diagnostics'], 'types': facts['types']}
        if mode == 'emit':
            every = set.intersection(*(set(run['emitted_files']) for run in runs))
            union = {}
            for run in runs:
                for path, digest in run['emitted_files'].items():
                    if union.setdefault(path, digest) != digest:
                        raise ValueError(f"{descriptor['name']}: the oracle emits {path} with different bytes")
            work[mode]['emitted'], work[mode]['emitted_sha256'] = common.files_digest(union)
            work[mode]['oracle_runs'] = len(runs)
            if varies:
                work[mode]['emitted_in_every_run'] = len(every)
            elif len(every) != len(union):
                raise ValueError(f"{descriptor['name']}: the oracle emitted {len(every)} to {len(union)} files across "
                                 f"{len(runs)} runs; investigate, then declare oracle_emit_varies with notes")
        common.require_unchanged(descriptor)
        print(f"{descriptor['name']} {mode}: exit {facts['exit']}, {facts['files']} files, "
              f"{facts['diagnostics']} diagnostics, {facts['types']} types"
              + (f", {work[mode]['emitted']} emitted over {len(runs)} runs, "
                 f"{work[mode].get('emitted_in_every_run', work[mode]['emitted'])} by every run" if mode == 'emit' else '')
              + f", {result['wall_ns'] / 1e9:.2f}s", flush=True)
    return work


def freeze(descriptor, oracle):
    descriptor = {**descriptor, 'work': oracle_work(descriptor, oracle)}
    common.validate(descriptor, descriptor['name'])
    path = common.SCENARIOS / f"{descriptor['name']}.json"
    path.write_text(json.dumps(descriptor, indent=2) + '\n')
    print(f'wrote {path.relative_to(common.ROOT)}')


def verify(descriptor, oracle):
    if 'work' not in descriptor:
        raise ValueError(f"{descriptor['name']}: not frozen")
    observed = oracle_work(descriptor, oracle)
    # A nondeterministic oracle's varying files and run count are themselves
    # samples; the required files, their bytes and everything else must agree.
    # A varying oracle's run count and every-run count are themselves samples;
    # its union, like everything else, must agree.
    ignored = {'types', 'oracle_runs', 'emitted_in_every_run'}
    differences = [f'{mode}.{field}: {observed[mode].get(field)!r}, frozen {descriptor["work"][mode][field]!r}'
                   for mode in common.MODES for field in sorted(descriptor['work'][mode])
                   if field not in ignored and observed[mode].get(field) != descriptor['work'][mode][field]]
    for difference in differences:
        print(f"{descriptor['name']} {difference}")
    return not differences


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('command', choices=('provision', 'freeze', 'verify'))
    parser.add_argument('names', nargs='*', help='scenarios (default: all five)')
    parser.add_argument('--clean', action='store_true', help='provision: start from an empty checkout')
    args = parser.parse_intermixed_args()
    try:
        descriptors = [common.load(name, refreeze=args.command == 'freeze') for name in (args.names or common.NAMES)]
        if args.command == 'provision':
            for descriptor in descriptors:
                common.provision(descriptor, clean=args.clean)
            return
        oracle = common.binaries()['go']
        if args.command == 'freeze':
            for descriptor in descriptors:
                freeze(descriptor, oracle)
        elif not all([verify(descriptor, oracle) for descriptor in descriptors]):
            sys.exit(1)
        else:
            print(f'{len(descriptors)} scenarios reproduce their frozen work')
    except subprocess.CalledProcessError as error:
        sys.exit(f'phase7_scenarios.py {args.command}: {error.cmd[0]} exited {error.returncode}')
    except (OSError, ValueError) as error:
        sys.exit(f'phase7_scenarios.py {args.command}: {error}')


if __name__ == '__main__':
    main()
