#!/usr/bin/env python3
"""E4 integration through the production checker, node builder and printer.

The existing S04 leaf and S05 scanner producers keep their own evidence scopes.
Every frozen case is evaluated by both runtimes; mismatches produce false metrics
and retained raw observations rather than silently selecting passing queries.
"""
import json
from pathlib import Path
import sys
import time

from s04 import verified_upstream
from s04_common import command, strict_json_loads
from s08_oracle import ROOT, canonical, digest, run_overlay
from s08_p5_display import DRIVER, validate

REQUESTS = ROOT / 'tools/s08/p5/text-requests.json'
COVERAGE = ROOT / 'data/s08/p5/text-coverage.json'
CASES = ROOT / 'data/s08/p5/text-cases.json'
CRITERIA = {'token_literal_bytes', 'helper_printer_semantics'}


def validate_inventory(request, coverage, cases):
    if coverage.get('version') != 1 or coverage.get('request_sha256') != digest(canonical(request) + b'\n'):
        raise ValueError('checker text request inventory drift; review and refreeze coverage')
    groups = coverage.get('groups', [])
    if not cases or len(set(cases)) != len(cases):
        raise ValueError('empty or duplicate checker text cases')
    if [g['id'] for g in groups] != cases or [p['id'] for p in request['programs']] != cases:
        raise ValueError('missing, reordered or unknown checker text case')
    seen_criteria = set()
    for group, program in zip(groups, request['programs'], strict=True):
        if set(group) != {'id', 'criteria', 'queries'}:
            raise ValueError('invalid checker text group')
        criteria, queries = group['criteria'], group['queries']
        if not criteria or len(set(criteria)) != len(criteria) or not set(criteria) <= CRITERIA:
            raise ValueError('invalid checker text criteria')
        if not queries or len(set(queries)) != len(queries) or queries != [q['id'] for q in program['queries']]:
            raise ValueError('missing, duplicate or unknown checker text query')
        seen_criteria.update(criteria)
    if seen_criteria != CRITERIA:
        raise ValueError('an E4 integration criterion has no probes')


def compare(request, coverage, cases, expected, actual):
    validate_inventory(request, coverage, cases)
    fingerprint = digest(canonical(request) + b'\n')
    for label, result in [('native', expected), ('Rust', actual)]:
        if result.get('request_sha256') != fingerprint:
            raise ValueError(label + ' observed different checker text requests')
        validate(request, result)
    groups, failures = {}, []
    for spec, native, rust in zip(coverage['groups'], expected['programs'], actual['programs'], strict=True):
        passed = True
        for wanted, observed in zip(native['queries'], rust['queries'], strict=True):
            # This integration inventory requires real emitted bytes. An absent
            # type node on both sides does not establish literal/printer parity.
            if wanted['state'] != 'content' or observed != wanted:
                passed = False
                failures.append({'case': spec['id'], 'query': wanted['id'], 'native': wanted, 'rust': observed})
        groups[spec['id']] = passed
    metrics = {}
    for criterion in sorted(CRITERIA):
        selected = [g for g in coverage['groups'] if criterion in g['criteria']]
        metrics[criterion] = bool(selected) and all(groups[g['id']] for g in selected)
        metrics[criterion + '_probes'] = sum(len(g['queries']) for g in selected)
    metrics.update(probes=sum(len(g['queries']) for g in coverage['groups']), failed_probes=len(failures))
    return {'metrics': metrics, 'tests': {case: 'pass' if groups[case] else 'fail' for case in cases}}, failures


def measure():
    request = strict_json_loads(REQUESTS.read_bytes())
    coverage = strict_json_loads(COVERAGE.read_bytes())
    cases = strict_json_loads(CASES.read_bytes())
    validate_inventory(request, coverage, cases)
    verified_upstream()
    source_paths = [REQUESTS, COVERAGE, CASES, DRIVER, Path(__file__),
                    ROOT / 'scripts/s08_p5_display.py', ROOT / 'scripts/s08_oracle.py',
                    ROOT / 'tools/s08/p5/display.rs']
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in source_paths}
    directory = ROOT / 'target/s08' / ('checkertext-' + str(time.time_ns()))
    directory.mkdir(parents=True, exist_ok=False)
    expected = run_overlay(directory / 'native', 'checker', DRIVER.read_text(), request, 'TestS08P5Display')
    output = directory / 'rust.json'
    command(['cargo', 'run', '--locked', '--package', 'ts_compiler', '--example', 'p5_display', '--',
             str(directory / 'native/requests.json'), str(output)], cwd=ROOT)
    actual = strict_json_loads(output.read_bytes())
    report, failures = compare(request, coverage, cases, expected, actual)
    verified_upstream()
    if sources != {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in source_paths}:
        raise ValueError('checker text adapter inputs changed during capture')
    (directory / 'sources.json').write_bytes(canonical(sources) + b'\n')
    (directory / 'failures.json').write_bytes(canonical(failures) + b'\n')
    (directory / 'report.json').write_bytes(canonical(report) + b'\n')
    print(f'checker text: {report["metrics"]["probes"]} probes, {len(failures)} mismatches; raw capture {directory}', file=sys.stderr)
    return report


if __name__ == '__main__':
    try:
        print(json.dumps(measure(), sort_keys=True))
    except (OSError, ValueError, RuntimeError) as error:
        print(f'checker text capture failed: {error}', file=sys.stderr)
        raise SystemExit(1) from error
