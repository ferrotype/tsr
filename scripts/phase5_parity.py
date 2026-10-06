"""Batch bridge to the carried Go assertions; no independent approval ledger."""
from __future__ import annotations
import importlib
import sys
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def adapter():
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    return importlib.import_module('tools.phase5.harness.runner')


def prepare(suite, explicit=None):
    if explicit:
        path = Path(explicit)
        if path.is_dir():
            path /= 'prepared.json'
        return json.loads(path.read_text())
    return adapter().prepare(suite, ROOT / 'target/phase5/parity' / suite)


def compare_parent(native, rust):
    """Keep raw failures and pinned skips; Rust cannot introduce its own skips.

    Missing native observations are failures even if the Rust parent passes.
    The root's native_state is the measured denominator, not a static skip list.
    """
    n = {row['id']: row for row in native}
    r = {row['id']: dict(row) for row in rust}
    parent = rust[0]['parent']
    if parent not in n or parent not in r:
        raise ValueError(f'missing terminal parent: {parent}')
    if any(row['parent'] != parent for row in native + rust):
        raise ValueError('mixed parents in comparison')
    if len(n) != len(native) or len(r) != len(rust):
        raise ValueError('duplicate observation')
    r[parent]['native_state'] = n[parent]['state']
    for key, row in r.items():
        expected = n.get(key)
        if row['state'] == 'skip' and (expected is None or expected['state'] != 'skip'):
            row.update(state='fail', reason='unexpected Rust skip: ' + row.get('reason', ''))
        elif expected is None and row['state'] == 'pass':
            row.update(state='fail', reason='observation absent from native execution')
        elif expected and expected['state'] == 'skip' and row['state'] != 'skip':
            row.update(state='fail', reason='Rust executed a pinned skipped observation')
    for key, expected in n.items():
        if key not in r:
            r[key] = {'id': key, 'parent': parent, 'state': 'fail',
                      'reason': 'native observation missing from Rust execution'}
    # Native failures cannot establish a passing Rust comparison. Keep them raw
    # alongside Rust output for diagnosis rather than treating them as skips.
    if any(row['state'] == 'fail' for row in native):
        r[parent].update(state='fail', reason='native reference test failed')
    elif r[parent]['state'] == 'fail' and not r[parent].get('reason'):
        r[parent]['reason'] = ('baseline mismatch' if any('/baseline/' in row['id'] and row['state'] == 'fail' for row in rust)
                               else 'native assertion failed against Rust')
    return list(r.values())


def run_group(suite, prepared, variants, local, timeout, publish):
    runner = adapter()
    local = Path(local)
    local.mkdir(parents=True, exist_ok=True)
    elapsed = {}
    # Bound command-line size and process memory; retain one Rust worker inside
    # each batch. Never build or start an npm/network operation in this interval.
    for index in range(0, len(variants), 128):
        group = variants[index:index + 128]
        stage = local / str(index // 128)
        native, _ = runner.run_batch(suite, prepared, group, stage / 'native', timeout, native=True)
        (stage / 'native.ndjson').write_text(''.join(json.dumps(row) + '\n' for row in native))
        by_parent = {}
        for row in native:
            by_parent.setdefault(row['parent'], []).append(row)
        def complete(rows):
            publish(compare_parent(by_parent[rows[0]['parent']], rows))
        rust, times = runner.run_batch(suite, prepared, group, stage / 'rust', timeout, on_complete=complete)
        (stage / 'rust.ndjson').write_text(''.join(json.dumps(row) + '\n' for row in rust))
        elapsed.update(times)
    return elapsed


def counts(rows):
    roots = {row['id']: row for row in rows if row['id'] == row.get('parent')}
    if any(root.get('native_state') not in ('pass', 'fail', 'skip') for root in roots.values()):
        raise ValueError('missing measured native state')
    executed = {key for key, root in roots.items() if root['native_state'] != 'skip'}
    failed = {row['parent'] for row in rows if row['state'] == 'fail'} & executed
    return {'executed': len(executed), 'failing': len(failed),
            'native_skips': len(roots) - len(executed),
            'reference_failures': sum(root['native_state'] == 'fail' for root in roots.values()), 'limit': len(executed) // 200}
