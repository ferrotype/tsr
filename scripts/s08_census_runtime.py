"""Allocation provenance in a separate, diagnostic Go runtime overlay.

Observe original malloc requests, including compiler-specialized entry points.
Never used by the normal or phase CPU binaries. SDK files are not modified.
"""
import hashlib
import re
from pathlib import Path
from s04_common import command
from s08_oracle import ROOT, canonical


def runtime_overlay(directory, replace, upstream, env):
    version = command(['go', 'version'], cwd=ROOT, env=env).decode().strip()
    if ' go1.27.1 ' not in version:
        raise ValueError('census allocation observer requires pinned Go 1.27.1')
    goroot = Path(command(['go', 'env', 'GOROOT'], cwd=ROOT, env=env).decode().strip())
    destination = directory / 'runtime-overlay'
    destination.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for name, expected in [('malloc.go', 1), ('malloc_generated.go', None)]:
        source = goroot / 'src/runtime' / name
        original = source.read_text()
        pattern = r'func (mallocgc(?:SmallScanNoHeaderSC\d+|SmallNoScanSC\d+|TinySC2)?)\(size uintptr, typ \*_type, needzero bool\) unsafe.Pointer \{'
        def instrument(match):
            return match.group(0).replace('unsafe.Pointer {', '(s08Result unsafe.Pointer) {') + '\n\tif s08AllocationActive.Load() != 0 { defer s08RecordAllocation(&s08Result, size, typ) }'
        patched, count = re.subn(pattern, instrument, original)
        if count != (expected if expected is not None else 14):
            raise ValueError(f'Go allocation entry inventory drift: {name}: {count}')
        target = destination / name
        target.write_text(patched)
        replace[str(source)] = str(target)
        manifest[name] = {'sha256': hashlib.sha256(original.encode()).hexdigest(), 'entry_points': count}
    for name, virtual in [
        ('runtime_allocations.go', goroot / 'src/runtime/s08_allocations.go'),
        ('runtime_bridge.go', upstream / 'tsc/internal/checker/s08_runtime_bridge.go'),
    ]:
        source = ROOT / 'tools/s08/oracle/families' / name
        target = destination / name
        target.write_bytes(source.read_bytes())
        if virtual.exists():
            raise ValueError(f'census overlay would replace {virtual}')
        replace[str(virtual)] = str(target)
    (destination / 'sdk.json').write_bytes(canonical(manifest) + b'\n')
