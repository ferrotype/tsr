#!/usr/bin/env python3
"""Opt-in PGO build/training experiment; run paired screens separately.

Requires the pinned Rust toolchain's llvm-tools component and provisioned
Phase 7 scenarios. Saves instrumented/optimized executables and failure records
outside target; does not record performance budgets or run benchmarks.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from tools.phase7.bench import common  # noqa: E402
import s07_benchmark as benchmark  # noqa: E402

TRAINING = (('vscode', 2), ('vscode', 8), ('self-compiler', 2))


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def source_identity():
    """Re-enumerate each time: additions, removals and bundled bytes count."""
    paths = set()
    for directory in (ROOT / 'crates', ROOT / '.cargo'):
        for current, directories, files in os.walk(directory):
            directories[:] = [name for name in directories if name not in ('target', '.git')]
            for name in files:
                path = Path(current, name)
                if path.suffix in ('.rs', '.toml', '.lock') or 'bundled' in path.relative_to(ROOT).parts:
                    paths.add(path)
    paths.update(ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'))
    paths.update((ROOT / 'tools/phase7/scenarios').glob('*.json'))
    paths.update(ROOT / 'tools/phase7/bench' / name for name in ('pgo.py', 'common.py'))
    paths.update(ROOT / 'scripts' / name for name in ('s07_benchmark.py', 's04_common.py'))
    return {str(path.relative_to(ROOT)): common.sha256(path.read_bytes()) for path in sorted(paths) if path.is_file()}


def environment():
    env = dict(os.environ)
    for key, value in env.items():
        if value and (key in ('RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'LLVM_PROFILE_FILE', 'CARGO_INCREMENTAL')
                      or key.startswith('CARGO_PROFILE_')
                      or 'RUSTFLAGS' in key or key.endswith(('RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'))):
            raise ValueError('unexpected compiler override: ' + key)
    return env


def toolchain(env):
    channel, host = benchmark.rust_native_toolchain(env, ROOT)
    rustc = subprocess.check_output(['rustc', '+' + channel, '-vV'], cwd=ROOT, env=env, text=True)
    sysroot = Path(subprocess.check_output(['rustc', '+' + channel, '--print', 'sysroot'],
                                         cwd=ROOT, env=env, text=True).strip())
    profdata = sysroot / 'lib/rustlib' / host / 'bin/llvm-profdata'
    if not profdata.is_file():
        raise ValueError('install llvm-tools for the pinned Rust toolchain before running PGO')
    version = subprocess.check_output([str(profdata), '--version'], env=env, text=True)
    rust_llvm = re.search(r'LLVM version: ([\d.]+)', rustc)
    tool_llvm = re.search(r'LLVM version ([\d.]+)', version)
    if not rust_llvm or not tool_llvm or rust_llvm[1] != tool_llvm[1]:
        raise ValueError('llvm-profdata does not match rustc LLVM version')
    return {'channel': channel, 'host': host, 'rustc': rustc,
            'profdata': str(profdata), 'profdata_version': version}


def artifact(messages):
    selected = benchmark.cargo_executable(messages, ROOT / 'crates/tsrust/Cargo.toml', 'tsrust', 'bin', [])
    return next(row for row in map(json.loads, messages.splitlines()) if row.get('executable') == str(selected))


def execute(command, cwd, env, prefix, timeout=1800):
    """Save raw output before a caller validates status or parses facts."""
    failed = None
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, start_new_session=os.name == 'posix')
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        status = process.returncode
    except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        failed = error
        # Cargo's rustc/linker descendants must stop before another stage can run.
        try:
            if os.name == 'posix':
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass
        stdout, stderr = process.communicate()
        status = None
    stdout = stdout.decode(errors='replace') if isinstance(stdout, bytes) else stdout
    stderr = stderr.decode(errors='replace') if isinstance(stderr, bytes) else stderr
    prefix.with_suffix('.stdout').write_text(stdout)
    prefix.with_suffix('.stderr').write_text(stderr)
    record = {'command': list(map(str, command)), 'cwd': str(cwd), 'exit': status, 'stdout': stdout, 'stderr': stderr}
    if failed:
        record['error'] = str(failed) or type(failed).__name__
    write(prefix.with_suffix('.json'), {key: value for key, value in record.items() if key not in ('stdout', 'stderr')})
    if failed:
        raise failed
    return record


def input_identity(root, listed):
    bundled = ROOT / 'crates/tsr_bundled/bundled/libs'
    entries = []
    for name in listed:
        if name.startswith('bundled:///libs/'):
            path = (bundled / name.removeprefix('bundled:///libs/')).resolve()
            if not path.is_relative_to(bundled.resolve()):
                raise ValueError('invalid bundled library path')
        else:
            path = Path(name)
        entries.append((name.replace(str(root) + '/', '@SCENARIO@/'), common.sha256(path.read_bytes())))
    return {'frozen_inputs_sha256': common.inputs_digest(root, listed),
            'content_sha256': common.sha256(json.dumps(entries).encode())}


def preflight(binary, descriptor, checkers, env, prefix):
    common.require_unchanged(descriptor)
    root = common.checkout(descriptor)
    args = [arg for arg in common.arguments(descriptor, 'check', checkers)
            if arg not in ('--listFiles', '--extendedDiagnostics')]
    result = execute([str(binary), *args, '--listFilesOnly'], root, env, prefix)
    if result['exit'] or result['stderr']:
        raise ValueError('training file-list preflight failed')
    listed = result['stdout'].splitlines()
    identity = input_identity(root, listed)
    if len(listed) != descriptor['work']['check']['listed'] or identity['frozen_inputs_sha256'] != descriptor['work']['check']['inputs_sha256']:
        raise ValueError('training inputs differ from the frozen scenario')
    args = [arg for arg in common.arguments(descriptor, 'check', common.ORACLE_CHECKERS)
            if arg not in ('--listFiles', '--extendedDiagnostics')]
    result = execute([str(binary), *args, '--showConfig'], root, env, prefix.with_name(prefix.name + '-options'))
    normalized = result['stdout'].replace(str(root) + '/', common.ROOT_TOKEN + '/')
    options = common.sha256(normalized.encode())
    if result['exit'] or result['stderr'] or options != descriptor['work']['check']['options_sha256']:
        raise ValueError('training options differ from the frozen scenario')
    return {**identity, 'options_sha256': options, 'listed_paths': listed}


def run(output, cache, training=TRAINING):
    output = Path(output).resolve()
    if 'target' in output.parts:
        raise ValueError('--output must be outside target')
    output.mkdir(parents=True)  # Existing experiments are never overwritten.
    manifest = {'complete': False, 'stage': 'setup', 'builds': [], 'training': []}
    previous_cache = common.CACHE
    common.CACHE = Path(cache).resolve()
    def save():
        write(output / 'manifest.json', manifest)
    def unchanged():
        if source_identity() != manifest['sources'] or benchmark.cargo_configuration(env) != manifest['cargo_configuration']:
            raise ValueError('source, bundled inputs or Cargo configuration changed during PGO')
    def build(name, flags):
        manifest['stage'] = name
        save()
        unchanged()
        command = ['cargo', '+' + settings['channel'], 'build', '--release', '--locked', '--target', settings['host'],
                   '-p', 'tsrust', '--bin', 'tsrust', '--message-format=json-render-diagnostics', *release]
        result = execute(command, ROOT, {**env, 'CARGO_ENCODED_RUSTFLAGS': '\x1f'.join(flags)}, output / name)
        record = {'name': name, 'command': command, 'rustflags': flags, 'exit': result['exit']}
        manifest['builds'].append(record)
        save()
        if result['exit']:
            raise ValueError(name + ' build failed')
        selected = artifact(result['stdout'])
        directory = output / name
        directory.mkdir()
        shutil.copy2(selected['executable'], directory / 'tsrust')
        record.update(cargo_artifact=selected, artifact=common.identity(directory / 'tsrust'))
        unchanged()
        save()
        return directory / 'tsrust'
    save()
    try:
        env = environment()
        release = benchmark.release_configuration(env, ROOT)  # Includes fat LTO and package overrides; rejects config flags/wrappers.
        settings = toolchain(env)
        manifest.update(toolchain=settings, sources=source_identity(), cargo_configuration=benchmark.cargo_configuration(env),
                        revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                        cache=str(common.CACHE), release_configuration=release)
        save()
        raw = output / 'raw'
        raw.mkdir()
        probes = output / 'preflight-raw'
        probes.mkdir()
        instrumented = build('instrumented', ['-Cprofile-generate=' + str(raw)])
        for name, checkers in training:
            manifest['stage'] = f'train-{name}-{checkers}'
            save()
            unchanged()
            descriptor = common.load(name)
            if not common.provisioned(descriptor):
                raise ValueError(name + ': provision the scenario before PGO')
            prefix = output / manifest['stage']
            probe_env = {**env, 'LLVM_PROFILE_FILE': str(probes / (prefix.name + '-%m-%p.profraw'))}
            before = preflight(instrumented, descriptor, checkers, probe_env, prefix.with_name(prefix.name + '-preflight'))
            record = {'scenario': name, 'checkers': checkers, 'inputs_before': before}
            manifest['training'].append(record)
            save()
            root = common.checkout(descriptor)
            result = execute([str(instrumented), *common.arguments(descriptor, 'check', checkers)], root,
                             {**env, 'LLVM_PROFILE_FILE': str(raw / (prefix.name + '-%m-%p.profraw'))}, prefix, timeout=600)
            record.update(command=result['command'], exit=result['exit'])
            facts = common.observe(result, root)
            record.update(facts=facts, issues=common.problems(facts, descriptor['work']['check'], 'check'))
            if result['stderr']:
                record['issues'].append('training emitted stderr')
            after = preflight(instrumented, descriptor, checkers, probe_env, prefix.with_name(prefix.name + '-after'))
            record['inputs_after'] = after
            if after != before or facts['listed_paths'] != before['listed_paths']:
                record['issues'].append('training inputs, options or file order changed')
            common.require_unchanged(descriptor)
            unchanged()
            save()
            if record['issues']:
                raise ValueError('training work differs: ' + '; '.join(record['issues']))
        manifest['stage'] = 'merge'
        save()
        profiles = sorted(raw.glob('*.profraw'))
        if not profiles or any(path.stat().st_size == 0 for path in profiles):
            raise ValueError('training produced no usable raw profiles')
        merged = output / 'merged.profdata'
        result = execute([settings['profdata'], 'merge', '-o', str(merged), *map(str, profiles)], ROOT, env, output / 'merge')
        if result['exit']:
            raise ValueError('profile merge failed')
        manifest.update(raw_profiles=[common.identity(path) for path in profiles], merged_profile=common.identity(merged))
        save()
        build('optimized', ['-Cprofile-use=' + str(merged), '-Cllvm-args=-pgo-warn-missing-function'])
        if common.identity(merged) != manifest['merged_profile']:
            raise ValueError('merged profile changed during optimized build')
        manifest.update(complete=True, stage='complete')
        save()
        return manifest
    except (Exception, KeyboardInterrupt) as error:
        manifest['error'] = str(error) or type(error).__name__
        save()
        raise
    finally:
        common.CACHE = previous_cache


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new experiment directory outside target')
    parser.add_argument('--cache', type=Path, default=common.CACHE, help='provisioned Phase 7 scenario cache')
    args = parser.parse_args(argv)
    try:
        run(args.output, args.cache)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + '\n')


if __name__ == '__main__':
    main()
