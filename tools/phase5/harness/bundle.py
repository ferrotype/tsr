"""Build once and relocate Phase 5 executable artifacts between CI checkouts."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import shutil

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SUITES = ('fourslash', 'lsp')


def _digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def export_bundle(prepared, output):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    for suite in SUITES:
        info = prepared[suite]
        files = {'binary': f'{suite}-tests', 'test2json': 'test2json', 'server': 'phase5_testserver'}
        hashes = {}
        for key, name in files.items():
            source = Path(info[key])
            destination = output / name
            digest = _digest(source)
            if destination.exists() and _digest(destination) != digest:
                raise ValueError(f'conflicting bundle executable: {name}')
            if source.resolve() != destination:
                shutil.copy2(source, destination)
            hashes[name] = digest
        portable = {key: info[key] for key in ('suite', 'compiled_sources', 'goos', 'goarch')}
        portable.update(bundle_version=1, **files, sha256=hashes)
        (output / f'{suite}.portable.json').write_text(json.dumps(portable, indent=2) + '\n')


def relocate_bundle(bundle, checkout):
    bundle, checkout = Path(bundle).resolve(), Path(checkout).resolve()
    result = {}
    for suite in SUITES:
        portable = json.loads((bundle / f'{suite}.portable.json').read_text())
        if portable.get('bundle_version') != 1 or portable.get('suite') != suite:
            raise ValueError('unsupported portable metadata or suite mismatch')
        prepared = {key: portable[key] for key in ('suite', 'compiled_sources', 'goos', 'goarch')}
        for key in ('binary', 'test2json', 'server'):
            path = (bundle / portable[key]).resolve()
            if not path.is_relative_to(bundle) or not path.is_file():
                raise ValueError(f'invalid bundled {key} path')
            if _digest(path) != portable['sha256'][portable[key]]:
                raise ValueError(f'bundled {key} bytes changed')
            # upload/download-artifact does not preserve executable permissions.
            path.chmod(path.stat().st_mode | 0o111)
            prepared[key] = str(path)
        prepared.update(stage=str(bundle), cwd=str(checkout / 'upstream/tsc'))
        if not Path(prepared['cwd']).is_dir():
            raise ValueError('destination checkout lacks upstream/tsc')
        manifest = bundle / f'{suite}.prepared.json'
        manifest.write_text(json.dumps(prepared, indent=2) + '\n')
        result[suite] = str(manifest)
    return result


def prepare_bundle(output, stage):
    from tools.phase5.harness import runner
    first = runner.prepare('fourslash', Path(stage) / 'fourslash')
    second = runner.prepare('lsp', Path(stage) / 'lsp',
                            prebuilt_server=first['server'], prebuilt_test2json=first['test2json'])
    export_bundle({'fourslash': first, 'lsp': second}, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    build = commands.add_parser('prepare')
    build.add_argument('--output', required=True)
    build.add_argument('--stage', required=True)
    move = commands.add_parser('relocate')
    move.add_argument('--bundle', required=True)
    move.add_argument('--checkout', default=str(ROOT))
    args = parser.parse_args()
    if args.command == 'prepare':
        prepare_bundle(args.output, args.stage)
    else:
        relocate_bundle(args.bundle, args.checkout)


if __name__ == '__main__':
    # Direct script invocation must make the repository package importable.
    import sys
    sys.path.insert(0, str(ROOT))
    main()
