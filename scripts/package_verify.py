#!/usr/bin/env python3
"""Build Cargo archives in an isolated extracted workspace; never publish anything."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

from package_assets import ROOT, publication_policy, run as check_assets


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def consumer_executable(build_log, manifest):
    """Select the consumer Cargo actually built, including target overrides."""
    executables = set()
    for line in build_log.read_text().splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue  # Cargo progress on stderr shares the retained build log.
        if (isinstance(message, dict) and message.get('reason') == 'compiler-artifact'
                and message.get('manifest_path') == str(manifest)
                and message.get('target', {}).get('name') == 'package_consumer'
                and message.get('target', {}).get('kind') == ['bin']
                and message.get('executable')):
            executables.add(message['executable'])
    if len(executables) != 1:
        raise ValueError('Cargo did not report exactly one package consumer executable')
    return Path(executables.pop())


def run(output):
    check_assets(True)
    rows = [r for r in publication_policy() if r['publish']]
    names = {r['name'] for r in rows}
    output.mkdir(parents=True, exist_ok=True)
    # A failed rerun must never leave an earlier passing summary in place.
    (output / 'verified.json').unlink(missing_ok=True)
    commands = []
    env = dict(os.environ, RUSTUP_TOOLCHAIN=tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel'])
    for name in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
        env.pop(name, None)

    def command(args, cwd, log):
        commands.append({'command': args, 'cwd': str(cwd), 'log': log})
        with (output / log).open('w') as stream:
            subprocess.run(args, cwd=cwd, env=env, stdout=stream, stderr=subprocess.STDOUT, check=True)

    # Actual normalized Cargo archives, followed by independent builds below.
    # No registry is created and --no-verify alone never counts as verification.
    package = ['cargo', 'package', '--registry', 'crates-io', '--allow-dirty', '--offline',
               '--no-verify', '--target-dir', str(output / 'archive-build')]
    for name in sorted(names):
        package += ['-p', name]
    command(package + ['--list'], ROOT, 'package-list.log')
    command(package, ROOT, 'package.log')
    isolated = Path(tempfile.mkdtemp(prefix='tsr-packages-')).resolve()
    packages = isolated / 'packages'
    packages.mkdir()
    archives = {}
    for row in rows:
        source = tomllib.loads((ROOT / row['manifest']).read_text())['package']
        basename = row['name'] + '-' + source['version']
        archive = output / 'archive-build/package' / (basename + '.crate')
        with tarfile.open(archive) as stream:
            stream.extractall(packages, filter='data')
        directory = packages / basename
        manifest = tomllib.loads((directory / 'Cargo.toml').read_text())
        p = manifest['package']
        if p['name'] != row['name'] or p['publish'] != ['crates-io'] or 'workspace' in manifest:
            raise ValueError('unexpected normalized package: ' + row['name'])
        for key in ('description', 'repository', 'license', 'readme', 'edition', 'rust-version'):
            if not isinstance(p.get(key), str):
                raise ValueError('unresolved package metadata: ' + row['name'] + ':' + key)
        for name in ('LICENSE', 'NOTICE', 'README.md'):
            if (directory / name).read_bytes() != (ROOT / Path(row['manifest']).parent / name).read_bytes():
                raise ValueError('packaged document differs: ' + name)
        for table in [manifest, *manifest.get('target', {}).values()]:
            for kind in ('dependencies', 'build-dependencies', 'dev-dependencies'):
                for key, dep in table.get(kind, {}).items():
                    if isinstance(dep, dict) and ('path' in dep or 'workspace' in dep):
                        raise ValueError('unresolved packaged dependency: ' + key)
        if row['name'] == 'tsr_compiler' and 's08_relater_prototype' in manifest.get('dev-dependencies', {}):
            raise ValueError('private relater dependency leaked into compiler archive')
        if row['name'] == 'tsr_wasm' and ('corpus' in manifest.get('features', {}) or 's10_corpus' in manifest.get('dependencies', {})):
            raise ValueError('private corpus dependency leaked into wasm archive')
        archives[row['name']] = {'sha256': digest(archive), 'bytes': archive.stat().st_size,
                                'directory': str(directory), 'files': sorted(str(p.relative_to(directory)) for p in directory.rglob('*') if p.is_file())}
    patch = '\n'.join(f'{name} = {{ path = {json.dumps(info["directory"])} }}' for name, info in sorted(archives.items()))
    (isolated / 'Cargo.toml').write_text('[workspace]\nresolver = "2"\nmembers = ["packages/*", "consumer"]\n\n[patch.crates-io]\n' + patch + '\n')
    shutil.copy2(ROOT / 'Cargo.lock', isolated / 'Cargo.lock')
    consumer = isolated / 'consumer'
    (consumer / 'src').mkdir(parents=True)
    shutil.copy2(ROOT / 'tools/packaging/consumer.rs', consumer / 'src/main.rs')
    deps = ['tsr_arena', 'tsr_ast', 'tsr_core', 'tsr_embed', 'tsr_jsstring', 'tsr_bundled', 'tsr_vfs', 'tsr_tsoptions', 'tsr_compiler', 'tsr_locale', 'tsr_diagnostics']
    (consumer / 'Cargo.toml').write_text('[package]\nname = "package_consumer"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n[dependencies]\n' + ''.join(f'{name} = "0.1.0"\n' for name in deps) + 'serde_json = "1"\n')
    # All overrides point only to archive contents. No package source or config
    # is copied from the checkout; the root lock preserves external versions.
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--offline', '--format-version', '1'], cwd=isolated, env=env))
    for package in metadata['packages']:
        if package['source'] is None and not Path(package['manifest_path']).is_relative_to(isolated):
            raise ValueError('dependency escaped isolated package tree: ' + package['manifest_path'])
    original = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
    locked = {(p['name'], p['version'], p.get('source'), p.get('checksum')) for p in original if 'source' in p}
    for p in tomllib.loads((isolated / 'Cargo.lock').read_text())['package']:
        if 'source' in p and (p['name'], p['version'], p['source'], p.get('checksum')) not in locked:
            raise ValueError('external dependency changed during packaging: ' + p['name'])
    target = output / 'build'
    command(['cargo', 'build', '--locked', '--offline', '--workspace', '--all-features', '--target-dir', str(target),
             '--message-format=json'], isolated, 'native.log')
    executable = consumer_executable(output / 'native.log', consumer / 'Cargo.toml')
    command([str(executable)], isolated, 'consumer.log')
    consumer_observation = json.loads((output / 'consumer.log').read_text())
    command(['cargo', 'build', '--locked', '--offline', '-p', 'tsr_embed', '--no-default-features', '--target-dir', str(target)], isolated, 'parser-only.log')
    pin = json.loads((ROOT / 'tools/s10/toolchains.json').read_text())
    env['CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS'] = pin['wasm_rustflags']
    for mode in ('parser', 'checker'):
        args = ['cargo', 'build', '--locked', '--offline', '--release', '-p', 'tsr_wasm', '--target', pin['wasm_target'], '--target-dir', str(target)]
        if mode == 'checker':
            args += ['--features', 'checker']
        command(args, isolated, 'wasm-' + mode + '.log')
        shutil.copy2(target / pin['wasm_target'] / 'release/tsr_wasm.wasm', output / ('wasm-' + mode + '.wasm'))
    result = {'version': 1, 'state': 'pass', 'isolated_workspace': str(isolated), 'archives': archives,
              'commands': commands, 'consumer_observation': consumer_observation,
              'registry_publish_dry_run': 'pending: sibling 0.1.0 releases not published',
              'archive_lockfiles': 'Cargo-generated; isolated workspace pins external dependencies from repository lockfile'}
    (output / 'verified.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f'Verified {len(rows)} Cargo archives; report: {output / "verified.json"}')
    # Keep the small extracted tree for inspection; no registry or upload step.


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/publishing-prep')
    args = parser.parse_args()
    try:
        run(args.output.resolve())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, str(error) + '\n')
