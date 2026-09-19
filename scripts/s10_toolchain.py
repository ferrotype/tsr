#!/usr/bin/env python3
"""Fetch the pinned Binaryen release, verify its archive, and print wasm-opt's path."""
import hashlib
import json
import platform
from pathlib import Path
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def install():
    pin = json.loads((ROOT / 'tools/s10/toolchains.json').read_text())
    key = platform.system() + '/' + platform.machine()
    if key not in pin['binaryen_archives']:
        raise ValueError('no reviewed Binaryen archive for ' + key)
    release = pin['binaryen_archives'][key]
    directory = ROOT / 'target/s10/toolchain'
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / release['url'].rsplit('/', 1)[1]
    if not archive.exists():
        temporary = archive.with_suffix('.download')
        urllib.request.urlretrieve(release['url'], temporary)
        if hashlib.sha256(temporary.read_bytes()).hexdigest() != release['sha256']:
            raise ValueError('Binaryen archive checksum differs; retained download for inspection')
        temporary.replace(archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != release['sha256']:
        raise ValueError('cached Binaryen archive changed')
    with tarfile.open(archive) as stream:
        stream.extractall(directory, filter='data')
    return directory / ('binaryen-version_' + pin['wasm_opt_version']) / 'bin/wasm-opt'


if __name__ == '__main__':
    print(install())
