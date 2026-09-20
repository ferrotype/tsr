#!/usr/bin/env python3
"""Build a calibrated current-layout backing traffic diagnostic from the exact frozen closure."""
import fcntl, io, json, shutil, subprocess, sys, tarfile, tempfile, tomllib
from pathlib import Path
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
FROZEN = ROOT / 'target/s07-bis/text-processing-clean-candidate'
FROZEN_SHA = 'bab5c54fc779b86ebdb1f2bd5a2289dcd7a4066612eb004ceb02e16e1c906fdc'
OUT = ROOT / 'target/s07-bis/allocation-traffic-build'
sys.path.insert(0, str(ROOT / 'tools/s07/performance-experiments'))
import runner
from s07_benchmark import (CACHE, native_environment, rust_native_toolchain,
                           release_configuration, cargo_configuration_paths, rust_executable, cargo_executable)
from s04_common import strict_json_loads


def save(path, value):
    runner.write_json(path, value)

def run(command, label, cwd, env, expected=0, timeout=None):
    save(OUT / (label + '.command.json'), command)
    with (OUT / (label + '.stdout')).open('wb') as stdout, (OUT / (label + '.stderr')).open('wb') as stderr:
        try:
            result = subprocess.run(command, cwd=cwd, env=env, stdout=stdout, stderr=stderr, timeout=timeout)
        except subprocess.TimeoutExpired:
            save(OUT / (label + '.exit.json'), {'timeout': True, 'limit_seconds': timeout})
            raise
    save(OUT / (label + '.exit.json'), {'returncode': result.returncode, 'expected_returncode': expected})
    if result.returncode != expected:
        raise RuntimeError(label + ' failed; retained stdout/stderr/exit')
    return (OUT / (label + '.stdout')).read_bytes()

helper_paths = ['rustfmt.toml', 'tools/s07/performance-experiments/runner.py', 'scripts/s07_benchmark.py', 'scripts/s04_common.py', 'scripts/s04_ownership.py']
adapter_before = {path.name:runner.digest(path) for path in HERE.iterdir() if path.is_file()}
helpers_before = {name:runner.digest(ROOT/name) for name in helper_paths}

with (CACHE / 's07-benchmark/measurement.lock').open('a+') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    reference = runner.validate_bundle(FROZEN, FROZEN_SHA)
    runner.validate_inputs(FROZEN, reference['expected_work'])
    OUT.mkdir(exist_ok=False); (OUT / 'artifacts').mkdir()
    source = OUT / 'source'
    shutil.copytree(FROZEN / 'source', source)
    for path in source.rglob('*'): path.chmod(0o755 if path.is_dir() else 0o644)
    source.chmod(0o755)
    original = runner.inventory(source)
    # Patches and exact changed-file checks are completed before execution.
    import apply
    receipt = apply.apply(source)
    changed = [source/name for name in receipt['after']]
    run(['rustfmt', '+1.97.1', '--edition', '2021', '--config-path', str(ROOT/'rustfmt.toml'), *map(str, changed)], 'rustfmt', source, native_environment())
    receipt['after_format'] = {str(path.relative_to(source)):runner.digest(path) for path in changed}
    save(OUT / 'apply-receipt.json', receipt)
    # Restore omitted workspace members from the exact source revision. Original
    # manifests/lock stay intact; these files only make Cargo metadata complete.
    members = tomllib.loads((source/'Cargo.toml').read_text())['workspace']['members']
    missing = [name for name in members if not (source/name/'Cargo.toml').is_file()]
    assert missing
    commit = '8f7236eaad350be329ef7ad3089bb5a13509c44e'
    fixtures = ['data/s07/ast-helper-observations.tsv', 'data/s07/diagnostic-order-observations.tsv',
                'data/s06/ast-utilities-middle-kinds.bin', 'data/s06/ast-utilities-middle-behaviors.tsv']
    assert all(not (source/name).exists() for name in fixtures)
    archive = run(['git', 'archive', commit, *missing, *fixtures], 'missing-workspace-source', ROOT, {})
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:') as contents:
        for member in contents:
            target = source/member.name
            assert target.resolve().is_relative_to(source) and not member.issym() and not member.islnk()
            if member.isdir(): target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                assert not target.exists()
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(contents.extractfile(member).read())
            else: raise ValueError('unexpected archive member')
    staged = runner.inventory(source)
    assert {name for name in original if original[name] != staged[name]} == set(receipt['before'])
    added_source = {name:staged[name] for name in staged.keys()-original.keys()}
    workspace_added = {name:entry for name,entry in added_source.items() if any(name.startswith(member+'/') for member in missing)}
    fixture_added = {name:added_source[name] for name in fixtures}
    diagnostic_added = {name:entry for name,entry in added_source.items() if name not in workspace_added and name not in fixture_added}
    assert set(diagnostic_added) == set(receipt['after']) - set(original)
    save(OUT/'added-source-inventory.json', added_source)
    env = native_environment()
    stable, host = rust_native_toolchain(env, source)
    assert stable == '1.97.1' and host == 'aarch64-apple-darwin'
    rustc = run(['rustc', '+'+stable, '-vV'], 'rustc', source, env).decode()
    config = lambda: {str(path): runner.digest(path) if path.is_file() else None for path in cargo_configuration_paths(env, source)}
    before_config = config()
    artifacts = {}; builds = {}
    for allocation, role in ((True, 'allocation'),):
        argv = ['cargo', '+'+stable, 'build', '--release', '--offline', '--locked', '--package', 'tsr_bench', '--bin', 'ts-bench', '--target', host, '--message-format=json-render-diagnostics', *release_configuration(env, source)]
        if allocation: argv += ['--features', 'allocation']
        with tempfile.TemporaryDirectory(prefix='allocation-traffic-build-', dir=ROOT/'target') as temporary:
            argv += ['--target-dir', temporary, '--config', 'build.build-dir='+json.dumps(temporary)]
            messages = run(argv, role+'-cargo', source, env)
            executable = rust_executable(messages, source/'crates/tsr_bench/Cargo.toml', allocation)
            assert executable.resolve().is_relative_to(Path(temporary).resolve())
            destination = OUT/'artifacts'/'rust-allocation-traffic'
            shutil.copyfile(executable, destination); destination.chmod(0o755)
            run([str(destination), str(OUT/'must-not-read-inputs.json'), '8'], 'worker-guard', source, env, expected=1, timeout=10)
            assert (OUT/'worker-guard.stdout').read_bytes()==b''
            assert b'name/table diagnostic requires one worker' in (OUT/'worker-guard.stderr').read_bytes()
            probe_argv = [arg for arg in argv]
            at = probe_argv.index('--bin'); probe_argv[at:at+2] = ['--example', 'allocation_traffic_probe']
            probe_messages = run(probe_argv, 'calibration-cargo', source, env)
            probe = cargo_executable(probe_messages, source/'crates/tsr_bench/Cargo.toml', 'allocation_traffic_probe', 'example', ['allocation'])
            assert probe.resolve().is_relative_to(Path(temporary).resolve())
            probe_destination = OUT/'artifacts'/'allocation-traffic-probe'
            shutil.copyfile(probe, probe_destination); probe_destination.chmod(0o755)
            run([str(probe_destination)], 'calibration', source, env, timeout=30)
            tests = ['cargo', '+'+stable, 'test', '--offline', '--locked', '--package', 'tsr_ast', '--lib', '--target', host, '--target-dir', temporary, '--config', 'build.build-dir='+json.dumps(temporary), 'symbol_tables::tests']
            run(tests, 'symbol-table-tests', source, env)
            parser_tests = tests.copy()
            parser_tests[parser_tests.index('tsr_ast')] = 'tsr_parser'
            for test_filter in ['list_buffer::', 'factory::tests', 'lists::tests']:
                parser_tests[-1] = test_filter
                run([*parser_tests, '--', '--test-threads=1'], test_filter.replace('::', '-')+'-tests', source, env)
            clippy = ['cargo', '+'+stable, 'clippy', '--offline', '--locked', '--package', 'tsr_bench', '--bin', 'ts-bench', '--example', 'allocation_traffic_probe', '--features', 'allocation', '--target', host, '--target-dir', temporary, '--config', 'build.build-dir='+json.dumps(temporary), '--', '-D', 'warnings']
            run(clippy, 'clippy', source, env)
        runner.runtime_libraries(destination, OUT/(role+'-runtime-libraries.txt'))
        rows = [strict_json_loads(line) for line in messages.splitlines() if line]
        observed = next(row for row in rows if row.get('reason') == 'compiler-artifact' and row.get('executable') == str(executable))
        artifacts[role] = {'path': str(destination.relative_to(OUT)), 'sha256': runner.digest(destination)}
        builds[role] = {'command': argv, 'compiler_artifact': observed}
        artifacts['calibration'] = {'path': str(probe_destination.relative_to(OUT)), 'sha256': runner.digest(probe_destination)}
        assert config() == before_config and runner.inventory(source) == staged, 'staged inputs/config changed'
        print(role+' build passed', flush=True)
    assert config() == before_config and runner.inventory(source) == staged
    shutil.copyfile(FROZEN/'inputs.json', OUT/'inputs.json')
    assert adapter_before=={path.name:runner.digest(path) for path in HERE.iterdir() if path.is_file()}
    assert helpers_before=={name:runner.digest(ROOT/name) for name in helper_paths}
    tools = {}
    for path in HERE.iterdir():
        if path.is_file():
            destination = OUT/'adapter'/path.name; destination.parent.mkdir(exist_ok=True)
            shutil.copyfile(path, destination); tools[path.name] = runner.digest(path)
    helpers = {name:runner.digest(ROOT/name) for name in helper_paths}
    for name in helper_paths: runner.copy_file(ROOT/name, OUT/'tool-snapshot'/name)
    manifest = {'version':1,'diagnostic_only':True,'kind':'allocation-traffic-build',
        'frozen_manifest_sha256':FROZEN_SHA,'expected_work':reference['expected_work'],
        'source_before_inventory':original,'staged_source_inventory':staged,
        'added_workspace_source_commit':commit,'added_workspace_members':missing,'added_workspace_source_inventory':workspace_added,'test_fixture_source_commit':commit,'test_fixture_inventory':fixture_added,'diagnostic_added_source_inventory':diagnostic_added,'added_source_inventory':added_source,
        'configuration':before_config,'rustc':rustc,'artifacts':artifacts,'builds':builds,
        'adapter_inventory':tools,'helper_inventory':helpers,'adapter_inventory_before':adapter_before,'helper_inventory_before':helpers_before,
        'profile':'same enforced native release, panic unwind, fat LTO, one codegen unit',
        'clippy_command':clippy,'tests_command':tests,'scope':'one-worker backing requests/replacements/releases and process phase windows; unclassified residual explicit; no CPU/RSS acceptance claim'}
    print('manifest '+runner.seal(OUT, manifest), flush=True)
