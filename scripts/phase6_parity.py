"""The `jsapi` suite's batch bridge: the pinned client's own tests, native first."""
from __future__ import annotations
import importlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent


def adapter():
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    return importlib.import_module('tools.phase6.jsapi.runner')


def prepare(suite, explicit=None):
    if explicit:
        path = Path(explicit)
        if path.is_dir():
            path /= 'prepared.json'
        return json.loads(path.read_text())
    return adapter().prepare(ROOT / 'target/phase6/parity' / suite)


def compare_parent(native, rust):
    """The L7 rules: Rust cannot introduce skips, observations absent from the
    native run fail, and a native failure fails the parent."""
    from phase5_parity import compare_parent as compare
    return compare(native, rust)


def run_group(suite, prepared, variants, local, timeout, publish):
    runner = adapter()
    local = Path(local)
    local.mkdir(parents=True, exist_ok=True)
    elapsed = {}
    for variant in variants:
        native, _ = runner.run_batch(suite, prepared, [variant], local / 'native', timeout, native=True)
        (local / 'native.ndjson').open('a').write(''.join(json.dumps(row) + '\n' for row in native))
        rust, times = runner.run_batch(suite, prepared, [variant], local / 'rust', timeout)
        (local / 'rust.ndjson').open('a').write(''.join(json.dumps(row) + '\n' for row in rust))
        publish(compare_parent(native, rust))
        elapsed.update(times)
    return elapsed


def counts(rows):
    """Cases the native run executed, and the ones that do not pass against Rust."""
    roots = {row['id']: row for row in rows if row['id'] == row.get('parent')}
    if any(root.get('native_state') not in ('pass', 'fail', 'skip') for root in roots.values()):
        raise ValueError('missing measured native state')
    cases = [row for row in rows if row['id'] != row.get('parent')]
    executed = [row for row in cases if row['state'] != 'skip' or row.get('reason', '').startswith('unexpected Rust skip')]
    failing = [row for row in cases if row['state'] == 'fail']
    return {'files': len(roots), 'cases': len(cases), 'executed': len(executed),
            'failing': len(failing), 'native_skips': len(cases) - len(executed),
            'reference_failures': sum(root['native_state'] == 'fail' for root in roots.values())}
