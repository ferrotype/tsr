#!/usr/bin/env python3
"""Build/capture/verify scoped untimed access traces; no acceptance metrics."""
import argparse
import fcntl
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE.parent))
import runner
import stage
import verify
from s04_common import command, strict_json_loads
from s07_benchmark import cargo_configuration_paths, cargo_executable, native_environment, release_configuration

BASE = ROOT / 'target/s07-bis/cp1-node-read-candidate'
BASE_SHA = '3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931'
PAYLOAD_LIMIT = 32 * 1024**3
COMPRESSED_LIMIT = 3 * 1024**3
TEST_SUPPORT = ('data/s07/ast-helper-observations.tsv', 'data/s07/diagnostic-order-observations.tsv',
                'data/s06/ast-utilities-middle-kinds.bin', 'data/s06/ast-utilities-middle-behaviors.tsv')
WORKLOAD_DECLARATIONS = ('data/s07/vscode-parse-options.json', 'data/s07/vscode-files.json')


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inventory(directory):
    return {str(p.relative_to(directory)): {'sha256': digest(p), 'bytes': p.stat().st_size}
            for p in sorted(directory.rglob('*')) if p.is_file() and p != directory / 'manifest.json'}


def seal(directory, record):
    record['inventory'] = inventory(directory)
    runner.write_json(directory / 'manifest.json', record)
    for path in sorted(directory.rglob('*'), reverse=True):
        path.chmod(0o555 if path.is_dir() or path.parent == directory / 'artifacts' else 0o444)
    directory.chmod(0o555)
    return digest(directory / 'manifest.json')


def tool_files():
    own = [p for p in HERE.rglob('*') if p.is_file() and p.suffix in {'.py', '.rs', '.json', '.toml', '.lock'}]
    # Keep the shared producer modules as a conservative diagnostic tool closure.
    # Their imports include lazy environment/build helpers; a hand-maintained
    # direct-import list missed active transitive modules in the first review.
    # This inventory is not a sprint evidence source glob or acceptance metric.
    helpers = [HERE.parent / 'runner.py', *(ROOT / 'scripts').glob('*.py')]
    return {str(p.relative_to(ROOT)): digest(p) for p in sorted(set(own + helpers + [ROOT / name for name in (*TEST_SUPPORT, *WORKLOAD_DECLARATIONS)]))}


def snapshot_tools(output):
    files = tool_files()
    for name, sha in files.items():
        runner.copy_file(ROOT / name, output / 'tool-snapshot' / name)
        if digest(output / 'tool-snapshot' / name) != sha:
            raise ValueError('tool changed during snapshot')
    return files


def registry_entries(path):
    return {(r['name'], r['version'], r['source']): r['checksum']
            for r in tomllib.loads(path.read_text())['package'] if 'source' in r}


def validate_build(directory, sha):
    if digest(directory / 'manifest.json') != sha:
        raise ValueError('build manifest changed')
    record = strict_json_loads((directory / 'manifest.json').read_bytes())
    if record.get('kind') != 's07_bis_scoped_access_trace_build' or record.get('diagnostic_only') is not True:
        raise ValueError('wrong trace build')
    if inventory(directory) != record['inventory']:
        raise ValueError('trace build inventory drift')
    if any(p.is_symlink() or p.stat().st_mode & 0o222 for p in (directory, *directory.rglob('*'))):
        raise ValueError('trace build must be immutable')
    for artifact in record['artifacts'].values():
        path = directory / artifact['path']
        if not path.resolve().is_relative_to(directory) or digest(path) != artifact['sha256'] or not os.access(path, os.X_OK):
            raise ValueError('trace/control executable changed')
    return record


def build(output):
    reference = runner.validate_bundle(BASE, BASE_SHA)
    runner.validate_inputs(BASE, reference['expected_work'])
    output.mkdir(parents=True, exist_ok=False)
    (output / 'artifacts').mkdir()
    before_tools = snapshot_tools(output)
    source = output / 'source'
    shutil.copytree(BASE / 'source', source)
    for path in (source, *source.rglob('*')):
        path.chmod(0o755 if path.is_dir() else 0o644)
    changed = stage.apply(source)
    # These existing cfg(test) includes are outside the frozen native benchmark
    # closure. Record their current bytes explicitly as unit-test support; they
    # are not substituted into production or the accepted benchmark artifact.
    for name in TEST_SUPPORT:
        runner.copy_file(ROOT / name, source / name)
    crate = source / 'tools/s07/performance-experiments/access-trace'
    for name in ('Cargo.toml', 'Cargo.lock', 'src/main.rs'):
        runner.copy_file(HERE / name, crate / name)
    registry = registry_entries(source / 'Cargo.lock')
    if any(registry.get(k) != value for k, value in registry_entries(crate / 'Cargo.lock').items()):
        raise ValueError('trace dependency not present in accepted lock')
    env = native_environment()
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target/s07-bis-access-trace-native')
    stable = tomllib.loads((source / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    rustc = command(['rustc', '+' + stable, '-vV'], cwd=crate, env=env).decode()
    host = next(line.removeprefix('host: ') for line in rustc.splitlines() if line.startswith('host: '))
    def config():
        return {str(p): digest(p) if p.is_file() else None for p in cargo_configuration_paths(env, crate)}
    before_config, before_source = config(), inventory(source)
    # Release tests avoid an additional debug dependency build on this disk-bound
    # diagnostic host. The artifact still uses the exact native release settings.
    test_args = ['cargo', '+' + stable, 'test', '--release', '--offline', '--locked',
                 '--manifest-path', str(crate / 'Cargo.toml'), '-p', 'tsr_ast',
                 '--lib', 'access_trace', '--', '--nocapture']
    runner.write_json(output / 'build-declaration.json', {
        'baseline_manifest_sha256': BASE_SHA, 'rustc': rustc, 'test_command': test_args,
        'configuration': before_config, 'tool_inputs': before_tools,
        'staged_source_inventory': before_source})
    tested = subprocess.run(test_args, cwd=crate, env=env, capture_output=True)
    (output / 'test.stdout').write_bytes(tested.stdout)
    (output / 'test.stderr').write_bytes(tested.stderr)
    runner.write_json(output / 'test-receipt.json', {'command': test_args, 'returncode': tested.returncode,
        'stdout_sha256': digest(output / 'test.stdout'), 'stderr_sha256': digest(output / 'test.stderr')})
    if tested.returncode:
        raise ValueError('trace recorder/state tests failed; inspect test.stderr')
    argv = ['cargo', '+' + stable, 'build', '--release', '--offline', '--locked',
            '--manifest-path', str(crate / 'Cargo.toml'), '--bin', 'tsr_s07_bis_access_trace',
            '--target', host, '--message-format=json-render-diagnostics', *release_configuration(env, crate)]
    built = subprocess.run(argv, cwd=crate, env=env, capture_output=True)
    (output / 'cargo-messages.ndjson').write_bytes(built.stdout)
    (output / 'cargo.stderr').write_bytes(built.stderr)
    runner.write_json(output / 'build-receipt.json', {'command': argv, 'returncode': built.returncode,
        'stdout_sha256': digest(output / 'cargo-messages.ndjson'), 'stderr_sha256': digest(output / 'cargo.stderr')})
    if built.returncode:
        raise ValueError('trace build failed; inspect cargo.stderr')
    binary = cargo_executable(built.stdout, crate / 'Cargo.toml', 'tsr_s07_bis_access_trace', 'bin', [])
    runner.copy_file(binary, output / 'artifacts/access-trace')
    runner.copy_file(BASE / reference['artifacts']['normal']['path'], output / 'artifacts/control')
    for artifact in ('access-trace', 'control'):
        (output / 'artifacts' / artifact).chmod(0o755)
        runner.runtime_libraries(output / 'artifacts' / artifact, output / (artifact + '-libraries.txt'))
    runner.copy_file(BASE / 'inputs.json', output / 'inputs.json')
    if before_config != config() or before_source != inventory(source) or before_tools != tool_files():
        raise ValueError('trace source/tools/configuration changed during build')
    return seal(output, {'version': 1, 'kind': 's07_bis_scoped_access_trace_build', 'diagnostic_only': True,
        'baseline_manifest_sha256': BASE_SHA, 'baseline_source_fingerprint': reference['source_fingerprint'],
        'expected_work': reference['expected_work'], 'rustc': rustc, 'tool_inputs': before_tools,
        'configuration': before_config, 'staged_source_inventory': before_source,
        'patch_sha256': digest(source / 'access-trace.patch'), 'patched_files': changed,
        'test_support': {name: digest(source / name) for name in TEST_SUPPORT},
        'test_command': test_args, 'build_command': argv,
        'artifacts': {name: {'path': 'artifacts/' + name, 'sha256': digest(output / 'artifacts' / name)}
                      for name in ('access-trace', 'control')}})


class CappedOutput:
    def __init__(self, stream, cap):
        self.stream, self.cap, self.written = stream, cap, 0

    def write(self, data):
        if self.written + len(data) > self.cap:
            raise ValueError('declared compressed trace cap exceeded; incomplete capture retained')
        self.stream.write(data)
        self.written += len(data)
        return len(data)

    def flush(self):
        self.stream.flush()


def graph_match(actual, control):
    count = 0
    with actual.open('rb') as left, control.open('rb') as right:
        for a in left:
            b = right.readline()
            if strict_json_loads(a) != strict_json_loads(b):
                raise ValueError(f'observer changed retained graph at input {count}; raw captures retained')
            count += 1
        if right.read(1):
            raise ValueError('observer/control graph counts differ')
    return count


def match_trace_work(trace, summary, graphs):
    rows = trace['files']
    if len(rows) != summary['files']:
        raise ValueError('trace file count differs from native summary')
    exclusive = sum(row['bound_in_place'] for row in rows)
    if exclusive != summary['bound_in_place_files'] or len(rows) - exclusive != summary['fallback_files']:
        raise ValueError('trace publication paths differ from native summary')
    with graphs.open('rb') as stream:
        for index, row in enumerate(rows):
            graph = strict_json_loads(stream.readline())
            if (row['file'], row['input_index'], graph['index']) != (index + 1, index, index):
                raise ValueError('trace and graph input order differ')
            for field, graph_field in (('source_bytes', 'source_bytes'), ('nodes', 'node_count'),
                                       ('symbols', 'symbol_count'), ('parse_diagnostics', 'parse_diagnostics'),
                                       ('bind_diagnostics', 'bind_diagnostics')):
                if row[field] != graph[graph_field]:
                    raise ValueError(f'trace/graph {field} differs at input {index}')
        if stream.read(1):
            raise ValueError('trace has fewer files than graph report')
    for field in ('parse_diagnostics', 'bind_diagnostics'):
        if sum(row[field] for row in rows) != summary[field]:
            raise ValueError('trace diagnostics differ from native summary')


def capture(build_dir, build_sha, output, sizing, payload_limit, compressed_limit, record_only=False):
    record = validate_build(build_dir, build_sha)
    runner.validate_inputs(build_dir, record['expected_work'])
    output.mkdir(parents=True, exist_ok=False)
    before_tools = snapshot_tools(output)
    inputs, recipes = runner.requests_from_frozen(build_dir / 'inputs.json')
    indices = list(range(len(inputs)))
    if sizing:
        # A declared diagnostic subset, never described as a workload capture or
        # performance sample: first16 and largest16 source files, in source order.
        largest = sorted(indices, key=lambda i: (-Path(inputs[i]['local']).stat().st_size, i))[:16]
        indices = sorted(set(indices[:16] + largest))
        inputs = [inputs[i] for i in indices]
    recipes = [recipes[i] for i in indices]
    selected_work = {'files': len(inputs), 'loaded_bytes': sum(row['source_bytes'] for row in recipes),
                     'loaded_input_sha256': runner.loaded_input_digest(recipes)}
    runner.write_json(output / 'inputs.json', inputs)
    runner.write_json(output / 'recipes.json', recipes)
    free = shutil.disk_usage(output).free
    if free < compressed_limit + 1024**3:
        raise ValueError('insufficient free space for declared compressed cap plus 1GiB reserve')
    declaration = {'version': 1, 'diagnostic_only': True, 'sizing_subset': sizing, 'input_indices': indices,
                   'payload_limit': payload_limit, 'compressed_limit': compressed_limit,
                   'free_bytes_before': free, 'minimum_free_reserve': 1024**3,
                   'gzip_level': 1, 'active_node_verifier_limit': 2_000_000,
                   'build_manifest_sha256': build_sha}
    runner.write_json(output / 'declaration.json', declaration)
    argv = [str(build_dir / 'artifacts/access-trace'), str(output / 'inputs.json'),
            str(output / 'summary.json'), str(output / 'graphs.ndjson'), str(payload_limit)]
    raw = output / 'trace.bin.gz'
    failed = None
    raw_bytes = 0
    with (output / 'child.stderr').open('xb') as stderr, raw.open('xb') as compressed:
        child = subprocess.Popen(argv, cwd=ROOT, env=native_environment(), stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=stderr)
        try:
            sink = CappedOutput(compressed, compressed_limit)
            with gzip.GzipFile(filename='', mode='wb', compresslevel=1, fileobj=sink, mtime=0) as stream:
                while chunk := child.stdout.read(1024 * 1024):
                    raw_bytes += len(chunk)
                    if raw_bytes > payload_limit + payload_limit // 1024 + 1024**2:
                        raise ValueError('binary framing exceeds declared stream bound')
                    stream.write(chunk)
                    if raw_bytes % (64 * 1024**2) < 1024**2 and shutil.disk_usage(output).free < 1024**3:
                        raise ValueError('trace disk reserve reached; incomplete capture retained')
        except BaseException as error:
            failed = str(error)
            child.kill()
        finally:
            child.stdout.close()
            status = child.wait()
    receipt = {'version': 1, 'build_manifest_sha256': build_sha, 'command': argv,
               'child_returncode': status, 'compression_error': failed, 'stream_bytes': raw_bytes,
               'trace_sha256': digest(raw), 'stderr_sha256': digest(output / 'child.stderr')}
    runner.write_json(output / 'capture-receipt.json', receipt)
    if status or failed:
        raise ValueError('trace capture incomplete; receipt and partial stream retained')
    summary = strict_json_loads((output / 'summary.json').read_bytes())
    if any(summary.get(key) != value for key, value in selected_work.items()):
        raise ValueError('native loaded inputs differ from the declared selection')
    if not sizing:
        for key, value in record['expected_work'].items():
            if summary.get(key) != value:
                raise ValueError('trace work differs: ' + key)
    # Semantic comparison uses the original accepted executable, same selected
    # inputs/options, all retained graph records, and no normalization waiver.
    control_args = [str(build_dir / 'artifacts/control'), str(output / 'inputs.json'), '1', '--graphs']
    with (output / 'control-graphs.ndjson').open('xb') as stdout, (output / 'control.stderr').open('xb') as stderr:
        control = subprocess.run(control_args, cwd=ROOT, env=native_environment(), stdout=stdout, stderr=stderr)
    runner.write_json(output / 'control-receipt.json', {'command': control_args, 'returncode': control.returncode})
    if control.returncode:
        raise ValueError('accepted graph control failed; raw failure retained')
    graphs = graph_match(output / 'graphs.ndjson', output / 'control-graphs.ndjson')
    if graphs != len(inputs) or summary['files'] != len(inputs):
        raise ValueError('trace omitted workload inputs')
    # The shared benchmark validator deliberately requires all13,094 inputs.
    # Keep that rule intact: revalidate the complete frozen parent, then compare
    # the native selected digest against the selected recipes derived above.
    runner.validate_inputs(build_dir, record['expected_work'])
    if record_only:
        if before_tools != tool_files():
            raise ValueError('recording tools changed')
        # Large traces may be verified by a separately recorded decoder. Seal
        # the raw invocation and graph checks, but make its pending state explicit
        # and incompatible with the verified-capture replay consumer.
        return seal(output, {'version': 1, 'kind': 's07_bis_access_trace_pending_verification',
            'diagnostic_only': True, 'trace_verified': False, 'build_manifest_sha256': build_sha,
            'declaration': declaration, 'receipt': receipt, 'summary': summary,
            'graph_matches': graphs, 'tool_inputs': before_tools,
            'limitations': ['Native recording and retained-graph checks completed; trace framing/value verification is still required.',
                            'No layout timing or sprint acceptance evidence.']})
    registries = [build_dir / 'tool-snapshot' / HERE.relative_to(ROOT) / (name + '-registry.json')
                  for name in ('protocol', 'state', 'hooks')]
    trace_report = verify_capture(raw, payload_limit, compressed_limit, registries=registries, expected={
        'files': summary['files'], 'source_bytes': summary['loaded_bytes'],
        'nodes': summary['nodes'], 'symbols': summary['symbols']},
        progress=lambda row: print(json.dumps({'verify_progress': row}), file=sys.stderr, flush=True),
        progress_records=5_000_000)
    match_trace_work(trace_report, summary, output / 'graphs.ndjson')
    runner.write_json(output / 'verification.json', trace_report)
    if before_tools != tool_files():
        raise ValueError('capture tools changed')
    return seal(output, {'version': 1, 'kind': 's07_bis_scoped_access_trace', 'diagnostic_only': True,
        'build_manifest_sha256': build_sha, 'declaration': declaration, 'receipt': receipt,
        'summary': summary, 'graph_matches': graphs, 'trace_verification': trace_report,
        'tool_inputs': before_tools, 'limitations': [
            'Source-level scoped operations, not hardware accesses or CPU coverage.',
            'This first capture does not reconstruct complete binding/payload state or rank storage layouts.',
            'No timing, allocation or RSS measurement; observer state export changes cache behavior.',
            'Absent operations outside the named hook inventory have unavailable counts.']})


def verify_capture(path, payload_limit, compressed_limit, registries=None, **options):
    # Kept as an explicit adapter to the independent framing verifier.
    if registries is None:
        registries = [HERE / (name + '-registry.json') for name in ('protocol', 'state', 'hooks')]
    return verify.verify_file(path, registries, max_payload=payload_limit, max_compressed=compressed_limit, **options)


def replay(directory, capture_sha, build_dir, build_sha):
    validate_build(build_dir, build_sha)
    if digest(directory / 'manifest.json') != capture_sha:
        raise ValueError('capture manifest differs from recorded digest')
    report = strict_json_loads((directory / 'manifest.json').read_bytes())
    if (report.get('kind') != 's07_bis_scoped_access_trace' or report.get('diagnostic_only') is not True
            or report['build_manifest_sha256'] != build_sha or inventory(directory) != report['inventory']):
        raise ValueError('capture provenance/inventory changed')
    if any(p.is_symlink() or p.stat().st_mode & 0o222 for p in (directory, *directory.rglob('*'))):
        raise ValueError('capture must remain immutable')
    receipt = strict_json_loads((directory / 'capture-receipt.json').read_bytes())
    if receipt != report['receipt'] or receipt['child_returncode'] != 0 or receipt['compression_error'] is not None:
        raise ValueError('capture child did not complete successfully')
    control = strict_json_loads((directory / 'control-receipt.json').read_bytes())
    if control['returncode'] != 0:
        raise ValueError('graph control failed')
    if graph_match(directory / 'graphs.ndjson', directory / 'control-graphs.ndjson') != report['graph_matches']:
        raise ValueError('graph comparison changed')
    declaration = report['declaration']
    summary = strict_json_loads((directory / 'summary.json').read_bytes())
    if summary != report['summary']:
        raise ValueError('native summary changed')
    registries = [build_dir / 'tool-snapshot' / HERE.relative_to(ROOT) / (name + '-registry.json')
                  for name in ('protocol', 'state', 'hooks')]
    verified = verify_capture(directory / 'trace.bin.gz', declaration['payload_limit'],
        declaration['compressed_limit'], registries=registries, expected={
            'files': summary['files'], 'source_bytes': summary['loaded_bytes'],
            'nodes': summary['nodes'], 'symbols': summary['symbols']})
    match_trace_work(verified, summary, directory / 'graphs.ndjson')
    if verified != report['trace_verification']:
        raise ValueError('trace replay differs from captured verification')
    return {'capture_manifest_sha256': capture_sha, 'files': summary['files'],
            'records': verified['records'], 'graph_matches': report['graph_matches'], 'replayed': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['build', 'capture', 'record', 'verify', 'replay'])
    parser.add_argument('--build', type=Path)
    parser.add_argument('--build-sha')
    parser.add_argument('--capture-sha')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sizing', action='store_true')
    parser.add_argument('--payload-limit', type=int, default=PAYLOAD_LIMIT)
    parser.add_argument('--compressed-limit', type=int, default=COMPRESSED_LIMIT)
    args = parser.parse_args()
    if args.payload_limit <= 0 or args.compressed_limit <= 0:
        raise ValueError('capture bounds must be positive')
    if args.command == 'verify':
        print(json.dumps(verify_capture(args.output.resolve(), args.payload_limit, args.compressed_limit), indent=2))
        return
    if args.command == 'replay':
        print(json.dumps(replay(args.output.resolve(), args.capture_sha,
                                args.build.resolve(), args.build_sha), indent=2))
        return
    lock_path = runner.CACHE / 's07-benchmark/measurement.lock'
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open('a+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            sha = build(args.output.resolve()) if args.command == 'build' else capture(
                args.build.resolve(), args.build_sha, args.output.resolve(), args.sizing,
                args.payload_limit, args.compressed_limit, record_only=args.command == 'record')
        except Exception as error:
            # Preserve a diagnostic even when the native child succeeded but a
            # later graph/format/provenance check rejected the capture.
            if args.output.is_dir() and not (args.output / 'manifest.json').exists():
                failure = args.output / 'failure.json'
                if not failure.exists():
                    runner.write_json(failure, {'command': args.command, 'error': str(error), 'accepted': False})
            raise
    print(json.dumps({'manifest_sha256': sha}))


if __name__ == '__main__':
    main()
