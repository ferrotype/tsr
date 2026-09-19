"""Checked observer additions to an accepted immutable source copy only."""
import difflib
import json
from pathlib import Path

import hooks
import state

HERE = Path(__file__).resolve().parent


def apply(directory):
    directory = Path(directory).resolve(strict=True)
    if directory == HERE.parents[3] or (directory / '.git').exists():
        raise ValueError('trace staging requires an isolated source copy')
    if directory.name == 'source' and (directory.parent / 'manifest.json').exists():
        raise ValueError('cannot instrument a sealed source bundle')
    if any(p.is_symlink() for p in directory.rglob('*')):
        raise ValueError('trace staging does not follow source symlinks')
    if (directory / 'access-trace.patch').exists():
        raise ValueError('access trace already staged')
    before = {str(p.relative_to(directory)): p.read_text()
              for p in directory.rglob('*')
              if p.is_file() and p.suffix in {'.rs', '.toml', '.lock'}}
    state.apply(directory)
    hooks.apply(directory)
    ast = directory / 'crates/tsr_ast'
    lib = ast / 'src/lib.rs'
    if 'pub mod access_trace;' in lib.read_text():
        raise ValueError('recorder module already present')
    lib.write_text(lib.read_text() + '\n// Isolated S07-bis observer; never applied to the production checkout.\npub mod access_trace;\n')
    (ast / 'src/access_trace.rs').write_text((HERE / 'src/recorder.rs').read_text())
    manifest = ast / 'Cargo.toml'
    original = manifest.read_text()
    if original.count('[dependencies]\n') != 1 or 'sha2' in original:
        raise ValueError('AST dependency declaration changed')
    manifest.write_text(original.replace('[dependencies]\n', '[dependencies]\nsha2 = "0.10"\n'))
    workspace = directory / 'Cargo.toml'
    lines = workspace.read_text().splitlines(keepends=True)
    selected = [i for i, line in enumerate(lines) if line.startswith('members = ')]
    if len(selected) != 1:
        raise ValueError('workspace member declaration changed')
    members = sorted(str(p.parent.relative_to(directory)) for p in (directory / 'crates').glob('*/Cargo.toml'))
    lines[selected[0]] = 'members = ' + json.dumps(members) + '\n'
    workspace.write_text(''.join(lines))
    after = {str(p.relative_to(directory)): p.read_text()
             for p in directory.rglob('*')
             if p.is_file() and p.suffix in {'.rs', '.toml', '.lock'}}
    changed = sorted(k for k in after if before.get(k) != after[k])
    patch = ''.join(line for name in changed for line in difflib.unified_diff(
        before.get(name, '').splitlines(keepends=True), after[name].splitlines(keepends=True),
        fromfile=name, tofile=name))
    (directory / 'access-trace.patch').write_text(patch)
    return changed
