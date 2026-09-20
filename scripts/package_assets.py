#!/usr/bin/env python3
"""Sync/check package assets against repository licenses and the upstream gitlink."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def publication_policy():
    rows = json.loads((ROOT / 'tools/packaging/packages.json').read_text())['packages']
    manifests = subprocess.check_output(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard', '**/Cargo.toml'],
        cwd=ROOT, text=True).splitlines()
    manifests = {p for p in manifests if 'package' in tomllib.loads((ROOT / p).read_text())}
    if len({r['name'] for r in rows}) != len(rows) or {r['manifest'] for r in rows} != manifests:
        raise ValueError('publication policy must name every package exactly once')
    by_name = {r['name']: r for r in rows}
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']
    if workspace['publish'] is not False:
        raise ValueError('workspace publication default must stay false')
    for row in rows:
        manifest = tomllib.loads((ROOT / row['manifest']).read_text())
        package = manifest['package']
        policy = package.get('publish', True)
        if isinstance(policy, dict):
            policy = workspace['publish']
        if package['name'] != row['name'] or policy != (['crates-io'] if row['publish'] else False):
            raise ValueError('manifest differs from publication policy: ' + row['name'])
        if not row['publish']:
            continue
        for field in ('description', 'repository', 'readme', 'license'):
            if not package.get(field):
                raise ValueError(f"missing {field}: {row['name']}")
        tables = [manifest, *manifest.get('target', {}).values()]
        for table in tables:
            for kind in ('dependencies', 'build-dependencies', 'dev-dependencies'):
                for key, dep in table.get(kind, {}).items():
                    if not isinstance(dep, dict) or 'path' not in dep:
                        continue
                    if kind == 'dev-dependencies' and 'version' not in dep:
                        continue  # Cargo drops path-only dev dependencies on publish.
                    name = dep.get('package', key)
                    if name not in by_name or not by_name[name]['publish'] or 'version' not in dep:
                        raise ValueError(f"unpublishable dependency: {row['name']} -> {name}")
    return rows


def expected_assets(rows):
    pin = json.loads((ROOT / 'data/upstream.json').read_text())['pin']
    gitlink = subprocess.check_output(['git', 'ls-files', '--stage', 'upstream'], cwd=ROOT, text=True)
    if gitlink.split()[:2] != ['160000', pin]:
        raise ValueError('upstream pin differs from gitlink')
    prefix = 'tsc/internal/bundled/'
    try:
        archive = subprocess.check_output(['git', '-C', str(ROOT / 'upstream'), 'archive', pin,
                                          prefix + 'CopyrightNotice.txt', prefix + 'libs'])
    except subprocess.CalledProcessError as error:
        raise ValueError('initialize upstream at the pinned gitlink before syncing/checking assets') from error
    bundled = {}
    with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
        for entry in stream.getmembers():
            if entry.isfile():
                name = entry.name.removeprefix(prefix)
                if name != 'CopyrightNotice.txt' and not re.fullmatch(r'libs/lib[\w.\-]*\.d\.ts', name):
                    raise ValueError('unexpected bundled asset: ' + name)
                bundled[name] = stream.extractfile(entry).read()
    source = (ROOT / 'crates/tsr_bundled/src/lib.rs').read_text()
    includes = re.findall(r'include_(?:bytes|str)!\(\s*"\.\./bundled/([^"]+)"\s*,?\s*\)', source)
    if sorted(includes) != sorted(bundled):
        raise ValueError('bundled include inventory differs from pinned upstream')
    library_pairs = re.findall(r'\(\s*"([^"]+)",\s*include_bytes!\(\s*"\.\./bundled/libs/([^"]+)"', source)
    if len(library_pairs) != len(bundled) - 1 or any(a != b for a, b in library_pairs):
        raise ValueError('bundled exported names differ from embedded asset names')
    expected = {'crates/tsr_bundled/bundled/' + name: data for name, data in bundled.items()}
    inventory = {'version': 1, 'upstream_pin': pin, 'upstream_directory': prefix.rstrip('/'),
                 'files': {name: {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                           for name, data in sorted(bundled.items())}}
    expected['crates/tsr_bundled/bundled/manifest.json'] = (json.dumps(inventory, indent=2) + '\n').encode()
    for row in rows:
        if row['publish']:
            directory = Path(row['manifest']).parent
            for name in ('LICENSE', 'NOTICE'):
                expected[str(directory / name)] = (ROOT / name).read_bytes()
    expected['crates/tsr_core/licenses/GO-BSD-3-Clause.txt'] = (ROOT / 'licenses/GO-BSD-3-Clause.txt').read_bytes()
    return expected


def run(check):
    rows = publication_policy()
    expected = expected_assets(rows)
    for relative, data in expected.items():
        path = ROOT / relative
        if check:
            if not path.is_file() or path.read_bytes() != data:
                raise ValueError('package asset drift: ' + relative)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    # Never silently retain removed libraries or ship unrelated files.
    extras = {str(p.relative_to(ROOT)) for p in (ROOT / 'crates/tsr_bundled/bundled').rglob('*')
              if p.is_file()} - set(expected)
    if extras:
        raise ValueError('remove obsolete bundled assets after reviewing: ' + ', '.join(sorted(extras)))
    print(f"{'Checked' if check else 'Synced'} {len(expected)} assets; "
          f"{sum(r['publish'] for r in rows)} public / {len(rows)} total packages")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='fail on drift without writing')
    args = parser.parse_args()
    try:
        run(args.check)
    except (ValueError, OSError) as error:
        parser.exit(1, str(error) + '\n')
