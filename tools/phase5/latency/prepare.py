#!/usr/bin/env python3
"""Prepare the proposed pinned TypeScript fixture offline; never build or fetch."""
import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
CONDITION = '@typescript/source'


def unpack_files(bundle, destination, prefix):
    for member in bundle.getmembers():
        relative = Path(member.name)
        if relative.is_absolute() or '..' in relative.parts or not (member.isfile() or member.isdir()):
            raise ValueError('Archive contains an unsafe path or member')
        if relative.parts[:len(prefix)] != prefix:
            # git archive includes ancestor directory entries.
            if member.isdir() and prefix[:len(relative.parts)] == relative.parts:
                continue
            raise ValueError('Archive member is outside the expected package root')
        target = destination.joinpath(*relative.parts[len(prefix):])
        if member.isdir():
            target.mkdir(parents=True, exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bundle.extractfile(member).read())


def unpack_locked(archive, destination, expected_name, locked):
    content = archive.read_bytes()
    integrity = 'sha512-' + base64.b64encode(hashlib.sha512(content).digest()).decode()
    if integrity != locked['integrity']:
        raise ValueError(f'{expected_name}: archive does not match pinned lock integrity')
    with tarfile.open(fileobj=io.BytesIO(content), mode='r:gz') as bundle:
        members = bundle.getmembers()
        roots = {Path(member.name).parts[0] for member in members if Path(member.name).parts}
        if len(roots) != 1:
            raise ValueError('Dependency archive requires one package root')
        unpack_files(bundle, destination, (next(iter(roots)),))
    package = json.loads((destination / 'package.json').read_text())
    if package.get('name') != expected_name or package.get('version') != locked['version']:
        raise ValueError(f'{expected_name}: archive package identity differs from lock')
    return {'version': locked['version'], 'resolved': locked['resolved'], 'integrity': integrity,
            'archive_sha256': hashlib.sha256(content).hexdigest()}


def prepare(node_archive, undici_archive, output):
    output = output.resolve()
    if output.exists():
        raise ValueError('Fixture output must be new; existing fixture bytes are never replaced')
    upstream = ROOT / 'upstream'
    pin = json.loads((ROOT / 'data/upstream.json').read_text())['pin']
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=upstream, text=True).strip()
    if actual != pin:
        raise ValueError('Fixture source checkout must match the recorded upstream pin')
    lock_bytes = subprocess.check_output(['git', 'show', pin + ':package-lock.json'], cwd=upstream)
    lock = json.loads(lock_bytes)['packages']
    source_bytes = subprocess.check_output(['git', 'archive', '--format=tar', pin, 'packages/typescript'], cwd=upstream)
    with tempfile.TemporaryDirectory(prefix='tsr-lsp-fixture-') as scratch:
        fixture = Path(scratch) / 'fixture'
        with tarfile.open(fileobj=io.BytesIO(source_bytes), mode='r:') as bundle:
            unpack_files(bundle, fixture, ('packages', 'typescript'))
        packages = {}
        for name, archive in (('@types/node', node_archive), ('undici-types', undici_archive)):
            packages[name] = unpack_locked(archive, fixture / 'node_modules' / name, name, lock['node_modules/' + name])
        config_path = fixture / 'tsconfig.json'
        config = json.loads(config_path.read_text())
        config['compilerOptions']['customConditions'] = [CONDITION]
        config_path.write_text(json.dumps(config, indent=2) + '\n')
        provenance = {'status': 'proposal; preparation does not approve the fixture', 'pin': pin,
                      'source': 'upstream/packages/typescript',
                      'lock_sha256': hashlib.sha256(lock_bytes).hexdigest(),
                      'source_archive_sha256': hashlib.sha256(source_bytes).hexdigest(),
                      'packages': packages, 'compiler_options_overlay': {'customConditions': [CONDITION]}}
        (fixture / 'latency-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(fixture, output, symlinks=True)
    return provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--node-archive', type=Path, required=True)
    parser.add_argument('--undici-archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    prepare(args.node_archive, args.undici_archive, args.output)


if __name__ == '__main__':
    main()
