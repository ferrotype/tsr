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


def built_executable(build_log, manifest, name):
    """Select the binary Cargo actually built, including target overrides."""
    executables = set()
    for line in build_log.read_text().splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue  # Cargo progress on stderr shares the retained build log.
        if (isinstance(message, dict) and message.get('reason') == 'compiler-artifact'
                and message.get('manifest_path') == str(manifest)
                and message.get('target', {}).get('name') == name
                and message.get('target', {}).get('kind') == ['bin']
                and message.get('executable')):
            executables.add(message['executable'])
    if len(executables) != 1:
        raise ValueError(f'Cargo did not report exactly one {name} executable')
    return Path(executables.pop())


# The installed command line, run from the extracted archives: the pin's
# version, one clean compile with --outDir and one checked type error.
CLI_VERSION = 'Version 7.1.0-dev\n'
CLI_SOURCE = 'declare const x: number;\nconst y: number = x + 1;\n'
CLI_EMIT = '"use strict";\nconst y = x + 1;\n'
CLI_ERROR_SOURCE = 'const s: string = 1;\n'
CLI_ERROR = "bad.ts(1,7): error TS2322: Type 'number' is not assignable to type 'string'.\n"


def cli_smoke(executable, directory, env):
    """Run the packaged tsrust the way a user would after cargo install."""
    directory.mkdir()
    def run(*args):
        return subprocess.run([str(executable), *args], cwd=directory, env=env, capture_output=True, text=True)
    version = run('--version')
    if version.returncode != 0 or version.stdout != CLI_VERSION or version.stderr:
        raise ValueError('packaged tsrust --version: ' + repr((version.returncode, version.stdout, version.stderr)))
    (directory / 'hello.ts').write_text(CLI_SOURCE)
    compiled = run('hello.ts', '--outDir', 'out')
    emitted = directory / 'out/hello.js'
    if (compiled.returncode != 0 or compiled.stdout or compiled.stderr or not emitted.is_file()
            or emitted.read_text() != CLI_EMIT or sorted(p.name for p in (directory / 'out').iterdir()) != ['hello.js']):
        raise ValueError('packaged tsrust compile: ' + repr((compiled.returncode, compiled.stdout, compiled.stderr)))
    (directory / 'bad.ts').write_text(CLI_ERROR_SOURCE)
    checked = run('bad.ts', '--outDir', 'out-error', '--pretty', 'false')
    # ExitStatus.DiagnosticsPresent_OutputsGenerated
    if checked.returncode != 2 or checked.stdout != CLI_ERROR or checked.stderr:
        raise ValueError('packaged tsrust type error: ' + repr((checked.returncode, checked.stdout, checked.stderr)))
    return {'executable': str(executable), 'version': version.stdout,
            'compile': {'source': CLI_SOURCE, 'arguments': ['hello.ts', '--outDir', 'out'],
                        'exit': compiled.returncode, 'emitted': {'out/hello.js': emitted.read_text()}},
            'type_error': {'source': CLI_ERROR_SOURCE, 'exit': checked.returncode, 'stdout': checked.stdout}}


def run(output):
    check_assets(True)
    policy = publication_policy()
    rows = [r for r in policy if r['publish']]
    names = {r['name'] for r in rows}
    private = {r['name'] for r in policy if not r['publish']}
    versions = {r['name']: tomllib.loads((ROOT / r['manifest']).read_text())['package']['version'] for r in rows}
    if len(set(versions.values())) != 1:
        raise ValueError('public packages are released at one version: ' + json.dumps(versions))
    release = versions[rows[0]['name']]
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
                    # Cargo drops path-only dev dependencies on private harnesses
                    # (s08_relater_prototype, tsr_contentmappertest, ...); none may remain.
                    if (dep.get('package', key) if isinstance(dep, dict) else key) in private:
                        raise ValueError(f"private dependency leaked into {row['name']} archive: {key}")
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
    (consumer / 'Cargo.toml').write_text('[package]\nname = "package_consumer"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n[dependencies]\n' + ''.join(f'{name} = "{versions[name]}"\n' for name in deps) + 'serde_json = "1"\n')
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
    # Extracted archives keep the packaged files' modification times, which
    # can predate an earlier run's artifacts, so Cargo would reuse stale
    # builds of changed packages. Every verification builds from scratch.
    shutil.rmtree(target, ignore_errors=True)
    command(['cargo', 'build', '--locked', '--offline', '--workspace', '--all-features', '--target-dir', str(target),
             '--message-format=json'], isolated, 'native.log')
    executable = built_executable(output / 'native.log', consumer / 'Cargo.toml', 'package_consumer')
    command([str(executable)], isolated, 'consumer.log')
    consumer_observation = json.loads((output / 'consumer.log').read_text())
    # The command line as cargo install builds it: default features only.
    command(['cargo', 'build', '--locked', '--offline', '-p', 'tsrust', '--bin', 'tsrust', '--target-dir', str(target),
             '--message-format=json'], isolated, 'tsrust.log')
    cli = built_executable(output / 'tsrust.log', Path(archives['tsrust']['directory']) / 'Cargo.toml', 'tsrust')
    cli_observation = cli_smoke(cli, isolated / 'cli-smoke', env)
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
              'release_version': release, 'commands': commands, 'consumer_observation': consumer_observation,
              'cli_observation': cli_observation,
              'registry_publish_dry_run': f'pending: sibling {release} releases not published',
              'archive_lockfiles': 'Cargo-generated; isolated workspace pins external dependencies from repository lockfile'}
    (output / 'verified.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f'Verified {len(rows)} Cargo archives at {release}; packaged tsrust ran {cli_observation["version"].strip()!r} '
          f'and compiled; report: {output / "verified.json"}')
    # Keep the small extracted tree for inspection; no registry or upload step.


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/publishing-prep')
    args = parser.parse_args()
    try:
        run(args.output.resolve())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, str(error) + '\n')
