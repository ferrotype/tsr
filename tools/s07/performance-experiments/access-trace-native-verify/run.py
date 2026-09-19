#!/usr/bin/env python3
"""Build or run the independent native counterpart of the scoped Python verifier."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
REFERENCE = HERE.with_name('access-trace') / 'verify.py'
SPEC = importlib.util.spec_from_file_location('native_verify_python_reference', REFERENCE)
reference = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = reference
SPEC.loader.exec_module(reference)
DEFAULT_PAYLOAD = 32 * 1024**3
DEFAULT_COMPRESSED = 3 * 1024**3


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n')


def inventory(directory):
    return {str(path.relative_to(directory)): {'bytes': path.stat().st_size, 'sha256': digest(path)}
            for path in sorted(directory.rglob('*')) if path.is_file() and path != directory / 'manifest.json'}


def copy(path, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(path, target)


def own_files():
    return sorted(path for path in HERE.rglob('*') if path.is_file() and path.suffix in {'.rs', '.py', '.toml', '.lock'})


def snapshot(paths, output):
    inputs = {str(path.relative_to(ROOT)): digest(path) for path in paths}
    for name, sha in inputs.items():
        copy(ROOT / name, output / name)
        require(digest(output / name) == sha, 'snapshot input changed')
    return inputs


def unchanged(inputs):
    require(all(digest(ROOT / name) == sha for name, sha in inputs.items()), 'source/helper input changed')


def seal(output, record):
    record['inventory'] = inventory(output)
    write_json(output / 'manifest.json', record)
    for path in sorted(output.rglob('*'), reverse=True):
        path.chmod(0o555 if path.is_dir() or path.parent.name == 'artifacts' else 0o444)
    output.chmod(0o555)
    return digest(output / 'manifest.json')


def captured_command(argv, cwd, env, output, name):
    result = subprocess.run(argv, cwd=cwd, env=env, capture_output=True)
    (output / (name + '.stdout')).write_bytes(result.stdout)
    (output / (name + '.stderr')).write_bytes(result.stderr)
    write_json(output / (name + '-receipt.json'), {'command': argv, 'cwd': str(cwd),
        'returncode': result.returncode, 'stdout_sha256': digest(output / (name + '.stdout')),
        'stderr_sha256': digest(output / (name + '.stderr'))})
    require(result.returncode == 0, name + ' failed; original stdout/stderr/receipt retained')
    return result.stdout


def build(output):
    sys.path.insert(0, str(ROOT / 'scripts'))
    from s07_benchmark import native_environment, release_configuration, cargo_executable, cargo_configuration_paths
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = output / 'source'
    allowed_lock = HERE.with_name('access-trace') / 'Cargo.lock'
    paths = [*own_files(), REFERENCE, allowed_lock, ROOT / 'rust-toolchain.toml', *sorted((ROOT / 'scripts').glob('*.py'))]
    inputs = snapshot(paths, output / 'tool-snapshot')
    for path in own_files():
        if path.suffix in {'.rs', '.toml', '.lock'}:
            copy(path, source / path.relative_to(HERE))
    # A subset of the already locked registry closure, without new versions.
    locked = lambda path: {(p['name'], p['version'], p['source']): p['checksum']
                          for p in tomllib.loads(path.read_text())['package'] if 'source' in p}
    allowed = locked(allowed_lock)
    require(all(allowed.get(key) == value for key, value in locked(source / 'Cargo.lock').items()),
            'native verifier introduced an unpinned registry dependency')
    env = native_environment()
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target/s07-bis-native-verify')
    stable = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    rustc = captured_command(['rustc', '+' + stable, '-vV'], source, env, output, 'rustc').decode()
    version = dict(line.split(': ', 1) for line in rustc.splitlines() if ': ' in line)
    require(version.get('release') == stable and version.get('host'), 'native compiler differs from pin')
    configurations = {str(path): digest(path) if path.is_file() else None
                      for path in cargo_configuration_paths(env, source)}
    profile = release_configuration(env, source)
    argv = ['cargo', '+' + stable, 'build', '--release', '--offline', '--locked', '--manifest-path',
            str(source / 'Cargo.toml'), '--target', version['host'], '--message-format=json-render-diagnostics', *profile]
    write_json(output / 'declaration.json', {'version': 1, 'command': argv, 'tool_inputs': inputs,
        'configuration': configurations, 'diagnostic_only': True})
    messages = captured_command(argv, source, env, output, 'cargo')
    binary = cargo_executable(messages, source / 'Cargo.toml', 'tsr_s07_access_trace_native_verify', 'bin', [])
    copy(binary, output / 'artifacts/native-verifier')
    (output / 'artifacts/native-verifier').chmod(0o755)
    if sys.platform == 'darwin':
        raw = captured_command(['otool', '-L', str(output / 'artifacts/native-verifier')], source, env, output, 'runtime-libraries')
        libraries = [line.strip().split(' (', 1)[0] for line in raw.decode().splitlines()[1:]]
        require(all(name.startswith(('/usr/lib/', '/System/Library/')) for name in libraries),
                'non-system native runtime dependency')
    elif sys.platform == 'linux':
        raw = captured_command(['ldd', str(output / 'artifacts/native-verifier')], source, env, output, 'runtime-libraries')
        libraries = [word for word in raw.decode().split() if word.startswith('/')]
        require(all(name.startswith(('/lib/', '/lib64/', '/usr/lib/', '/usr/lib64/')) for name in libraries),
                'non-system native runtime dependency')
    else:
        raise ValueError('unsupported native diagnostic host')
    unchanged(inputs)
    require(configurations == {str(path): digest(path) if path.is_file() else None
                               for path in cargo_configuration_paths(env, source)}, 'Cargo configuration changed')
    return seal(output, {'version': 1, 'kind': 's07_access_trace_native_verifier_build',
        'diagnostic_only': True, 'artifact': {'path': 'artifacts/native-verifier',
        'sha256': digest(output / 'artifacts/native-verifier')}, 'tool_inputs': inputs,
        'configuration': configurations, 'rustc': rustc})


def validate_build(directory, sha):
    directory = Path(directory).resolve()
    require(digest(directory / 'manifest.json') == sha, 'native build manifest differs')
    record = reference.strict_json((directory / 'manifest.json').read_bytes())
    require(record.get('kind') == 's07_access_trace_native_verifier_build' and record.get('diagnostic_only') is True,
            'wrong native build kind')
    require(inventory(directory) == record['inventory'], 'native build inventory changed')
    require(all(not path.is_symlink() and not path.stat().st_mode & 0o222
                for path in (directory, *directory.rglob('*'))), 'native build must be immutable')
    binary = directory / record['artifact']['path']
    require(binary.resolve().is_relative_to(directory) and digest(binary) == record['artifact']['sha256']
            and os.access(binary, os.X_OK), 'native binary differs')
    return binary, record


def verify_gzip(binary, binary_sha, trace, registries, output, *, config=None,
                compressed_limit=DEFAULT_COMPRESSED, build_manifest_sha256=None, trace_sha256=None):
    """Preserve success or failure receipts. Accept only native status+gzip+SHA agreement."""
    config = {} if config is None else dict(config)
    payload_limit = config.get('max_payload', DEFAULT_PAYLOAD)
    require(type(payload_limit) is int and payload_limit >= 0, 'invalid payload cap')
    require(type(compressed_limit) is int and compressed_limit >= 0, 'invalid compressed cap')
    output, binary, trace = Path(output).resolve(), Path(binary).resolve(), Path(trace).resolve()
    require(digest(binary) == binary_sha, 'native executable hash differs')
    output.mkdir(parents=True, exist_ok=False)
    tools = snapshot([*own_files(), REFERENCE], output / 'tool-snapshot')
    write_json(output / 'config.json', config)
    config_sha256 = digest(output / 'config.json')
    local_registries = []
    original_registries = []
    require(len(registries) == 3, 'three registries required')
    for index, path in enumerate(registries):
        path = Path(path).resolve()
        target = output / f'registry-{index}.json'
        copy(path, target)
        local_registries.append(target)
        original_registries.append({'path': str(path), 'sha256': digest(target)})
    argv = [str(binary), str(output / 'config.json'), *map(str, local_registries)]
    write_json(output / 'declaration.json', {'version': 1, 'diagnostic_only': True, 'command': argv,
        'binary_sha256': binary_sha, 'build_manifest_sha256': build_manifest_sha256,
        'input_trace': str(trace), 'expected_trace_sha256': trace_sha256,
        'registries': original_registries, 'config': config, 'config_sha256': config_sha256,
        'compressed_limit': compressed_limit,
        'tool_inputs': tools})
    failure, returncode, raw_bytes = None, None, 0
    reader = None
    with trace.open('rb') as source, (output / 'child.stdout').open('xb') as stdout, (output / 'child.stderr').open('xb') as stderr:
        child = None
        reader = reference.GzipReader(source, compressed_limit)
        try:
            child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr)
            while chunk := reader.read(reference.READ_CHUNK):
                raw_bytes += len(chunk)
                # At most one52-byte block header per52-byte record plus magic/footer.
                require(raw_bytes <= 2 * payload_limit + 68, 'uncompressed framing bound exceeded')
                child.stdin.write(chunk)
            child.stdin.close()
            returncode = child.wait(timeout=60)
        except BaseException as error:
            failure = str(error)
            if child is not None:
                child.kill()
                try:
                    child.stdin.close()
                except (BrokenPipeError, OSError):
                    pass
                returncode = child.wait()
    receipt = {'version': 1, 'diagnostic_only': True, 'command': argv, 'returncode': returncode,
        'wrapper_error': failure, 'child_started': child is not None,
        'accepted': False, 'gzip_complete': reader.finished,
        'compressed_bytes': reader.compressed_bytes, 'compressed_sha256': reader.compressed_sha.hexdigest(),
        'stream_bytes': raw_bytes, 'stream_sha256': reader.stream_sha.hexdigest(),
        'binary_sha256': binary_sha, 'build_manifest_sha256': build_manifest_sha256,
        'stdout_sha256': digest(output / 'child.stdout'), 'stderr_sha256': digest(output / 'child.stderr')}
    write_json(output / 'receipt.json', receipt)
    try:
        require(returncode == 0 and failure is None and reader.finished, 'native/gzip verification failed; receipt retained')
        require((output / 'child.stdout').stat().st_size <= 64 * 1024**2, 'native JSON report exceeds64MiB')
        report = reference.strict_json((output / 'child.stdout').read_bytes())
        require(type(report) is dict and report.get('version') == 1 and report.get('complete') is True
                and report.get('format') == 'S07TRC01' and report.get('full_semantic_replay') is False,
                'native report contract differs')
        require(report.get('stream_sha256') == receipt['stream_sha256'], 'native/decompressor raw stream hash differs')
        require(trace_sha256 is None or receipt['compressed_sha256'] == trace_sha256, 'compressed trace differs from declared identity')
        require('compressed_bytes' not in report and 'compressed_sha256' not in report, 'native report claims gzip facts')
        require(digest(binary) == binary_sha, 'native executable changed during verification')
        require(digest(output / 'config.json') == config_sha256, 'native verifier configuration changed')
        require(all(digest(path) == row['sha256'] for path, row in zip(local_registries, original_registries)), 'registry snapshot changed')
        unchanged(tools)
        report.update(compressed_bytes=receipt['compressed_bytes'], compressed_sha256=receipt['compressed_sha256'])
        write_json(output / 'verification.json', report)
        receipt['accepted'] = True
        receipt['verification_sha256'] = digest(output / 'verification.json')
        write_json(output / 'receipt.json', receipt)
        return report
    except BaseException as error:
        receipt['validation_error'] = str(error)
        write_json(output / 'receipt.json', receipt)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    builder = commands.add_parser('build')
    builder.add_argument('output', type=Path)
    verifier = commands.add_parser('verify')
    verifier.add_argument('build', type=Path)
    verifier.add_argument('build_sha256')
    verifier.add_argument('trace', type=Path)
    verifier.add_argument('output', type=Path)
    verifier.add_argument('--registry', type=Path, nargs=3, required=True)
    verifier.add_argument('--trace-sha256', required=True)
    verifier.add_argument('--config', type=Path)
    verifier.add_argument('--compressed-limit', type=int, default=DEFAULT_COMPRESSED)
    args = parser.parse_args()
    if args.command == 'build':
        print(build(args.output))
    else:
        binary, record = validate_build(args.build, args.build_sha256)
        config = reference.strict_json(args.config.read_bytes()) if args.config else {}
        report = verify_gzip(binary, record['artifact']['sha256'], args.trace, args.registry, args.output,
            config=config, compressed_limit=args.compressed_limit, build_manifest_sha256=args.build_sha256,
            trace_sha256=args.trace_sha256)
        sha = seal(args.output.resolve(), {'version': 1, 'kind': 's07_access_trace_native_verification',
            'diagnostic_only': True, 'build_manifest_sha256': args.build_sha256,
            'trace_sha256': report['compressed_sha256'], 'verification_sha256': digest(args.output / 'verification.json'),
            'reference_equivalence_scope': 'The same framed operation checks; no new semantic coverage.'})
        print(sha)


if __name__ == '__main__':
    main()
