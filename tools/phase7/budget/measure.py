#!/usr/bin/env python3
"""Serial one-checker resource capture over frozen v2 scenario inputs.

One discarded warm-up pair precedes seven alternating pairs, extended once
(to fourteen) by the S07 elapsed uncertainty rule. Busy rows and failed partial
captures are retained. Eligibility is independent of passing budget thresholds.

Elapsed is execute request -> acknowledgement (RPC included). Preparation,
checkpoint GC, and observation are excluded. Checkpoint process peak covers
launch through checkpoint, retained RSS is host ps after checkpoint, and the
entire lifecycle peak (including observation) is a separate diagnostic metric.
Startup/module compilation has no separate init acknowledgement and is missing.

Native: --runtime native --go BINARY --rust BINARY --go-build BUILD_JSON
        --rust-build BUILD_JSON --inventory DIR --contents DIR --input FILE
        --selection SELECTION_JSON --output NEW_DIR
Wasm: --runtime wasm --go ARTIFACT_DIR --rust ARTIFACT_DIR, with explicit
      manifests as above. Node, wasm, bindings, shim, and helpers are bound.

Prototype manifests stay diagnostic. Production budget manifests require
contract 2, reviewed source guards and release/allocator/GC evidence; generic
production checker wasm builds also require their actual row exports. Eligible
work binds the full committed scenario descriptor and provisioned selection.
The readonly perf.py adapter authenticates raw observations and recomputes all
metrics. Native retained heap requires a separate instrumented capture; RSS
never substitutes for it. A complete diagnostic capture exits zero, and a
failed capture exits two. Neither result silently accepts a missing budget.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from tools.phase7.budget import probe, stream, snapshot, scenario_input  # noqa: E402
from tools.phase7.bench.capture import BUSY_OTHER_CPU, other_cpu
from tools.phase7.bench import common  # noqa: E402
from s07_benchmark_measure import host_info, reject_concurrent_builds  # noqa: E402
from s07_benchmark_stats import ratio_summary  # noqa: E402

PAIRS = (7, 14)
GC_POLICY = 'v8_after_prepare_and_at_checkpoint'
PEAK_AUTHORITY = 'getrusage(RUSAGE_SELF).ru_maxrss'
ALLOCATOR = 'mimalloc-0.1.48'
GO_GC_POLICY = 'runtime.GC after prepare/prepare_end and checkpoint'
METRICS = ('elapsed_rpc_ns', 'checkpoint_process_peak_rss_bytes',
           'retained_process_rss_bytes', 'entire_lifecycle_peak_rss_bytes')


def positive(value, label):
    if type(value) is not int or not 0 < value < 2**64:
        raise ValueError(f'{label} must be a positive integer')
    return value


def bound_asset(path, expected):
    actual = probe.identity(path)
    if (not isinstance(expected, dict) or type(expected.get('bytes')) is not int
            or expected['bytes'] != actual['bytes'] or expected.get('sha256') != actual['sha256']):
        raise ValueError(f'artifact differs from build manifest: {path}')
    return actual


def artifact_entry(manifest, name):
    entries = [value for path, value in manifest.get('artifacts', {}).items() if Path(path).name == name]
    if len(entries) != 1:
        raise ValueError(f'build manifest must identify exactly one {name}')
    return entries[0]


def command_pair(command, flag, value):
    return isinstance(command, list) and any(part == flag and command[i + 1] == value
                                           for i, part in enumerate(command[:-1]))


def require_budget_manifest(manifest, engine):
    if (manifest.get('mode') != 'phase7-budget' or type(manifest.get('measurement_contract')) is not int
            or manifest['measurement_contract'] != 2 or manifest.get('performance_eligible') is not True
            or manifest.get('source_guard') is not True or type(manifest.get('build_exit')) is not int
            or manifest['build_exit'] != 0 or not manifest.get('sources')):
        raise ValueError('production budget build lacks contract 2/source guard/successful build evidence')
    if engine == 'go' and manifest.get('go_gc_policy') != GO_GC_POLICY:
        raise ValueError('production Go budget build has an unknown GC contract')


def build_identity(runtime, engine, artifact, build):
    """Recognize actual producer kinds; a filename or purpose label is no proof."""
    build = build.resolve(strict=True)
    manifest = probe.decode(build.read_bytes())
    if not isinstance(manifest, dict):
        raise ValueError('build manifest must be an object')
    files = [probe.identity(build)]
    reasons = []
    result = {'manifest': files[0], 'runtime': runtime, 'engine': engine, 'eligibility_reasons': reasons}
    environment = {}
    if engine == 'go':
        if manifest.get('mode') not in ('phase7-budget-prototype', 'phase7-budget'):
            raise ValueError('Go build must identify a supported budget producer')
        if manifest['mode'] == 'phase7-budget-prototype':
            if manifest.get('performance_eligible') is not False:
                raise ValueError('cannot upgrade a prototype build eligibility')
            reasons.append('Go build explicitly declares performance_eligible=false')
        else:
            require_budget_manifest(manifest, engine)
        pin_file = ROOT / 'data/upstream.json'
        if manifest.get('pin') != probe.decode(pin_file.read_bytes())['pin']:
            raise ValueError('Go artifact does not use the repository upstream pin')
        pin_identity = probe.identity(pin_file)
        if manifest.get('sources', {}).get('data/upstream.json') != pin_identity['sha256']:
            raise ValueError('Go artifact does not bind its upstream pin source')
        files.append(pin_identity)
        # This known implementation settles both ordinary and streamed prepare,
        # and checkpoint. The digest binds this source contract to built bytes;
        # OS peak availability alone does not prove the GC schedule.
        for name in ('operation.go', 'stream.go'):
            relative = 'tools/phase7/budget/go/body/' + name
            source = probe.identity(ROOT / relative)
            if manifest.get('sources', {}).get(relative) != source['sha256']:
                raise ValueError('Go build lacks the reviewed settled-prepare/checkpoint GC contract: ' + name)
            files.append(source)
        result['go_gc_policy'] = GO_GC_POLICY + '; source-digest bound'
    if runtime == 'native':
        executable = artifact.resolve(strict=True)
        if engine == 'go':
            files.append(bound_asset(executable, artifact_entry(manifest, 'budget-go')))
        else:
            command = manifest.get('build_command')
            if (not isinstance(command, list) or len(command) < 2 or command[:2] != ['cargo', 'build']
                    or not command_pair(command, '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml')):
                raise ValueError('Rust manifest is not the standalone public budget consumer build')
            build_exit = manifest.get('build_exit', manifest.get('validation', {}).get('build', {}).get('exit'))
            if (manifest.get('source_guard') is not True or manifest.get('source_changes_during_build') != []
                    or type(build_exit) is not int or build_exit != 0):
                raise ValueError('Rust build did not pass its source guard/build')
            if manifest.get('allocation_instrumented') is not False or manifest.get('allocator') != ALLOCATOR:
                raise ValueError('native timing requires uninstrumented known mimalloc')
            files.append(bound_asset(executable, {'bytes': manifest.get('size'), 'sha256': manifest.get('sha256')}))
            expected_profile = {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1}
            if (manifest.get('profile') != expected_profile or
                    type(manifest.get('profile', {}).get('codegen_units')) is not int):
                reasons.append('Rust build lacks the ordinary release fat-LTO/one-CGU/unwind profile')
            result['profile'] = manifest.get('profile')
            if manifest.get('mode') == 'phase7-budget':
                require_budget_manifest(manifest, engine)
            else:
                reasons.append('Rust build manifest has no supported production-budget mode')
        result['executable'] = str(executable)
    else:
        directory = artifact.resolve(strict=True)
        if os.environ.get('NODE_OPTIONS'):
            raise ValueError('explicit wasm capture requires NODE_OPTIONS unset; no hidden Node overrides')
        if not directory.is_dir() or build != directory / 'build.json':
            raise ValueError('wasm build manifest must be the selected artifact directory build.json')
        if engine == 'go':
            names = ('budget-go.wasm', 'wasm_exec.cjs')
            launcher = ROOT / 'tools/phase7/budget/go/wasm-consumer'
            helpers = [launcher, launcher.with_name('wasm.mjs')]
            environment['TSR_GO_WASM_ARTIFACT_DIR'] = str(directory)
        else:
            command = manifest.get('command')
            if (manifest.get('mode') != 'checker' or not command_pair(command, '-p', 'tsr_wasm')
                    or not isinstance(command, list)
                    or not any(part == '--features' and isinstance(command[i + 1], str)
                               and 'checker' in command[i + 1].split(',') for i, part in enumerate(command[:-1]))):
                raise ValueError('wasm Rust build must be the production tsr_wasm checker')
            if '--release' not in command or not manifest.get('sources'):
                raise ValueError('production checker requires a source-guarded ordinary release build')
            names = ('checker_bg.wasm', 'bindings.mjs')
            launcher = ROOT / 'tools/phase7/budget/production-wasm-consumer'
            helpers = [launcher, launcher.with_name('production-wasm.mjs'),
                       ROOT / 'tools/s10/wasm/instance.mjs', ROOT / 'tools/s10/wasm/imports.mjs']
            environment['TSR_WASM_ARTIFACT_DIR'] = str(directory)
        files += [bound_asset(directory / name, artifact_entry(manifest, name)) for name in names]
        result['helpers'] = [probe.identity(path) for path in helpers]
        files += result['helpers']
        node = shutil.which('node')
        if node is None:
            raise ValueError('Node is required for the explicit wasm runtime')
        pin_paths = [ROOT / 'upstream/package.json', ROOT / 'tools/s10/toolchains.json']
        files += [probe.identity(path) for path in pin_paths]
        expected_node = 'v' + probe.decode(pin_paths[0].read_bytes())['volta']['node']
        node_identity = probe.identity(node)
        files.append(node_identity)
        node_version = subprocess.check_output([node, '--version'], text=True, timeout=10).strip()
        if node_version != expected_node:
            raise ValueError(f'wasm capture requires pinned Node {expected_node}, found {node_version}')
        if probe.decode(pin_paths[1].read_bytes())['node_flags'] != ['--stack-size=4096']:
            raise ValueError('Node launcher flags differ from the pin')
        result.update(executable=str(launcher), node=node_identity,
                      node_version=node_version,
                      gc_policy=GC_POLICY, node_arguments=['--expose-gc', '--stack-size=4096'])
        # The shell launcher resolves Node from PATH. Pin PATH's first entry to
        # the bound executable's directory and check resolution on every guard.
        environment['PATH'] = str(Path(node).absolute().parent) + os.pathsep + os.environ.get('PATH', '')
    if engine == 'go':
        environment['GOGC'] = '100'
    environment.update(GOMEMLIMIT=None, GOEXPERIMENT=None, NODE_OPTIONS=None)
    result['files'] = files
    result['environment'] = environment
    result['production_eligible'] = not reasons
    result['contract_sources'] = [item for item in files if Path(item['path']).is_relative_to(ROOT)
                                  and item['path'] != str(build)
                                  and Path(item['path']).suffix in ('.go', '.json')]
    return result


def unchanged(identity):
    for expected in identity['files']:
        if probe.identity(expected['path']) != expected:
            raise ValueError('build, artifact, runtime, or helper changed during capture: ' + expected['path'])
    if identity['runtime'] == 'wasm':
        node = shutil.which('node', path=identity['environment']['PATH'])
        if node is None or probe.identity(node) != identity['node']:
            raise ValueError('Node launcher resolution changed during capture')


@contextmanager
def launch_environment(values):
    old = {name: os.environ.get(name) for name in values}
    for name, value in values.items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value
    try:
        yield
    finally:
        for name, value in old.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def sample_metrics(record, backend, operation):
    if record.get('complete') is not True or record.get('exit') != 0:
        raise ValueError('consumer capture did not complete successfully')
    probe.validate_observation(record['observation'], operation)
    checkpoint = record['checkpoint']
    probe.validate_checkpoint(checkpoint, backend)
    commands = record['commands']
    execute = [row for row in commands if row.get('command') == 'execute']
    names = [row.get('command') for row in commands]
    if (len(execute) != 1 or names.count('prepare_begin') != 1 or names.count('prepare_end') != 1
            or names.count('checkpoint') != 1 or names.count('observe_begin') != 1 or names.count('release') != 1
            or not names.index('prepare_begin') < names.index('prepare_end') < names.index('execute')
            < names.index('checkpoint') < names.index('observe_begin') < names.index('release')):
        raise ValueError('capture does not follow the single-checker streamed measurement schedule')
    if backend in ('go', 'rust'):
        if checkpoint.get('process_peak_rss_authority') != PEAK_AUTHORITY:
            raise ValueError('modern native checkpoint process peak is required')
        peak = checkpoint.get('process_peak_rss_before_observation_bytes')
        if backend == 'rust' and (checkpoint.get('allocation_instrumented') != 'false'
                                  or checkpoint.get('allocator') != ALLOCATOR):
            raise ValueError('native timing requires uninstrumented known mimalloc')
    else:
        if checkpoint.get('gc_policy') != GC_POLICY:
            raise ValueError('modern Node checkpoint with the known GC policy is required')
        peak = checkpoint.get('process_peak_rss_bytes')
    probe.decimal(peak, 'checkpoint process peak', positive=True)
    values = {'elapsed_rpc_ns': positive(execute[0].get('round_trip_ns'), 'execute RPC interval'),
              'checkpoint_process_peak_rss_bytes': int(peak),
              'retained_process_rss_bytes': positive(record.get('retained_process_rss_before_observation'), 'retained process RSS'),
              'entire_lifecycle_peak_rss_bytes': positive(record.get('entire_process_peak_rss_bytes'), 'lifecycle peak RSS')}
    if values['entire_lifecycle_peak_rss_bytes'] < values['checkpoint_process_peak_rss_bytes']:
        raise ValueError('lifecycle peak is smaller than checkpoint process peak')
    return values


def verify_contents(archive):
    # Archive.unchanged checks metadata/receipt identity. Blobs are verified
    # while preparing, and again after each sample to catch a change during
    # execute (including the final sample, with no following preparation).
    seen = set()
    for row in archive.content_rows:
        with stream.open_blob(archive.contents / 'blobs' / row['sha256']) as source:
            if os.fstat(source.fileno()).st_size != int(row['raw_size']):
                raise ValueError('frozen content changed size during capture')
            if row['sha256'] not in seen and hashlib.file_digest(source, 'sha256').hexdigest() != row['sha256']:
                raise ValueError('frozen content changed bytes during capture')
        seen.add(row['sha256'])
    archive.unchanged()


def summaries(pairs, runtime='native'):
    return {metric: ratio_summary([pair['go'][metric] for pair in pairs],
                                  [pair['rust'][metric] for pair in pairs],
                                  timing=metric == 'elapsed_rpc_ns',
                                  threshold=0.70 if metric == 'checkpoint_process_peak_rss_bytes' and runtime == 'native' else 1.0,
                                  accepted_counts=PAIRS)
            for metric in METRICS}


def elapsed_qualification_reasons(summary):
    reasons = []
    if summary['go_relative_mad'] > 0.05 or summary['rust_relative_mad'] > 0.05:
        reasons.append('elapsed relative MAD exceeds 5% after the fixed sampling rule')
    interval = summary['bootstrap']
    if interval['lower'] <= interval['threshold'] < interval['upper']:
        reasons.append('elapsed confidence interval remains inconclusive after the fixed sampling rule')
    return reasons


def archive_helpers(output, controllers, identities):
    """Keep the exact transport/controller bytes independently of build.json."""
    sources = {item['path']: item for item in controllers}
    for identity in identities.values():
        sources.update((item['path'], item) for item in identity.get('helpers', []) + identity.get('contract_sources', []))
    manifest = []
    for item in sources.values():
        relative = Path(item['path']).relative_to(ROOT)
        archived = output / 'helpers' / relative
        archived.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item['path'], archived)
        actual = probe.identity(archived)
        if any(actual[key] != item[key] for key in ('bytes', 'sha256')):
            raise ValueError('helper changed while archiving: ' + item['path'])
        manifest.append(dict(item, archived=str(archived.relative_to(output))))
    probe.write(output / 'helper-manifest.json', manifest)
    return manifest


def validate_selection(selection, descriptor, input_value):
    name, mode = selection.get('scenario'), selection.get('mode')
    if name not in common.NAMES or mode not in common.MODES:
        raise ValueError('unknown scenario/mode selection')
    common.validate(descriptor, name)
    if (type(selection.get('checkers')) is not int or selection['checkers'] != 1
            or selection.get('output_is_in_memory') is not True
            or selection.get('provisioned', {}).get('spec_sha256') != common.provision_spec(descriptor)):
        raise ValueError('selection is not a provisioned one-checker in-memory operation')
    scope = Path(os.fsdecode(bytes(input_value['current_directory'])))
    expected_name = f"{name}-{descriptor['source']['commit'][:12]}"
    if scope.name != expected_name:
        raise ValueError('selection scope is not the provisioned scenario checkout')
    # The five current committed descriptors all select project directories.
    # Reject an unrepresented config alias rather than silently relabel it.
    config = scope / descriptor['project'] / 'tsconfig.json'
    arguments = [value.replace('@OUT@', f'/phase7-output/{name}/{mode}') for value in descriptor['modes'][mode]]
    expected = {'current_directory': list(os.fsencode(scope)), 'case_sensitive': True, 'entries': [],
                'configuration': {'config_path': list(os.fsencode(config)),
                                  'arguments': [list(value.encode()) for value in arguments]},
                'operation': 'check' if mode == 'check' else 'check_emit'}
    if type(input_value.get('case_sensitive')) is not bool or input_value != expected:
        raise ValueError('input does not select the committed scenario project/mode/options')


def bind_selection(archive, path):
    selection = probe.decode(path.read_bytes())
    descriptor_path = common.SCENARIOS / (selection['scenario'] + '.json')
    raw = descriptor_path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != selection.get('descriptor_sha256'):
        raise ValueError('selection descriptor changed')
    descriptor = probe.decode(raw)
    validate_selection(selection, descriptor, archive.input)
    scope = Path(os.fsdecode(bytes(archive.input['current_directory'])))
    old_cache = scenario_input.common.CACHE
    try:
        expected, actual = scenario_input.make(selection['scenario'], selection['mode'], scope)
    finally:
        common.CACHE = old_cache
    if expected != archive.input or actual['provisioned'] != selection['provisioned']:
        raise ValueError('selection does not match the actual provisioned workload')
    root, inventory, _ = snapshot.load_inventory(archive.inventory)
    if root != os.fsencode(scope):
        raise ValueError('inventory scope differs from selected scenario')
    snapshot.verify_unchanged(root, inventory)
    return {'selection': selection, 'descriptor': descriptor, 'descriptor_source': probe.identity(descriptor_path),
            'selection_source': probe.identity(path), 'inventory_identity_sha256': inventory['identity_sha256'],
            'input_sha256': archive.identity['input_sha256'],
            'validation': 'scenario_input.make + complete snapshot OS identity at capture; replay uses archives only'}


def emitted_union(observation, selection):
    prefix = f"/phase7-output/{selection['scenario']}/{selection['mode']}/".encode()
    files = {}
    for row in observation['writes']:
        name = bytes(row['name'])
        if not name.startswith(prefix):
            raise ValueError('emitted file is outside the selected in-memory output')
        relative = name[len(prefix):].decode('utf-8')
        if not relative or Path(relative).is_absolute() or '..' in Path(relative).parts or relative in files:
            raise ValueError('noncanonical or duplicate emitted output')
        files[relative] = hashlib.sha256(bytes(row['text'])).hexdigest()
    return common.files_digest(files)


def checking_work(observation, binding, baseline):
    types = int(observation['types'])
    if baseline is not None and types < common.TYPES_FLOOR * baseline:
        raise ValueError('checking work fell below half its runtime one-checker warm-up')
    if binding is not None:
        work = binding['descriptor']['work'][binding['selection']['mode']]
        # These frozen scenarios' source rosters match the ordinary CLI file
        # count. Do not classify config files by extension: any supplementary
        # diagnostic source changes this complete roster and is refused here.
        if len(observation['sources']) != work['files'] or len(observation['diagnostics']) != work['diagnostics']:
            raise ValueError('source/diagnostic counts differ from frozen scenario work')
    return {'types': types, 'warmup_types': types if baseline is None else baseline,
            'minimum_fraction': common.TYPES_FLOOR,
            'authority': 'each runtime one-checker warm-up; four-checker CLI aggregate is not a type baseline'}


def work_comparison(reference, observed, engine, binding):
    checking_work(reference, binding, None)
    checking_work(observed, binding, None)
    raw = probe.comparison(reference, observed)
    result = {'differences': raw, 'omitted_write_names': [], 'omitted_emitted_names': []}
    if binding is None or binding['selection']['mode'] != 'emit' or not binding['descriptor'].get('oracle_emit_varies'):
        if raw:
            raise ValueError('owned observation differs from Rust reference: ' + ', '.join(raw))
        return result
    selection, descriptor = binding['selection'], binding['descriptor']
    work = descriptor['work']['emit']
    if emitted_union(reference, selection) != (work['emitted'], work['emitted_sha256']):
        raise ValueError('Rust emitted reference does not match the frozen descriptor union')
    if engine == 'rust':
        if raw:
            raise ValueError('Rust owned observation differs from its exact union reference: ' + ', '.join(raw))
        return result
    left = dict(reference, writes=[], emitted_files=[])
    right = dict(observed, writes=[], emitted_files=[])
    remaining = probe.comparison(left, right)
    if remaining:
        raise ValueError('non-emit observation differs: ' + ', '.join(remaining))
    for key, output in [('writes', 'omitted_write_names'), ('emitted_files', 'omitted_emitted_names')]:
        def name(row):
            return bytes(row['name'] if key == 'writes' else row)
        expected = {name(row): row for row in reference[key]}
        current = [name(row) for row in observed[key]]
        if len(expected) != len(reference[key]) or len(set(current)) != len(current):
            raise ValueError('oracle omission policy requires unique output names')
        current_names = set(current)
        missing = [name(row) for row in reference[key] if name(row) not in current_names]
        if len(missing) > common.ORACLE_OMISSIONS * len(expected):
            raise ValueError('Go omitted more than the descriptor oracle allowance')
        filtered = [row for row in reference[key] if name(row) in current_names]
        # Reuse exact normalized byte equality and preserve shared write order.
        if key == 'writes':
            mismatch = probe.comparison(dict(reference, writes=filtered, emitted_files=[]),
                                        dict(reference, writes=observed[key], emitted_files=[]))
        else:
            mismatch = [] if filtered == observed[key] else [key]
        if mismatch:
            raise ValueError('Go changed an emitted file byte/name/order outside the oracle allowance')
        result[output] = [list(item) for item in missing]
    result['policy'] = 'descriptor oracle_emit_varies; Go subset <=1%; shared bytes and order exact'
    return result


def require_output(archive, output):
    """Reject capture writes inside the source workload or frozen archives."""
    output = Path(output).resolve()
    if output.exists() or 'target' in output.resolve().parts:
        raise ValueError('require a new output outside target')
    protected = [Path(value).resolve() for value in
                 (getattr(archive, 'inventory', None), getattr(archive, 'contents', None)) if value is not None]
    if 'current_directory' in archive.input:
        protected.append(Path(os.fsdecode(bytes(archive.input['current_directory']))).resolve())
    if any(output.is_relative_to(path) for path in protected):
        raise ValueError('output must be outside the source workload and frozen archives')


def run(archive, identities, output, timeout, *, input_path=None, selection=None):
    """Capture only; all subprocess work is delegated to probe.capture."""
    require_output(archive, output)
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('require a finite positive command timeout')
    runtime = identities['go']['runtime']
    if identities['rust']['runtime'] != runtime:
        raise ValueError('paired consumers must use the same runtime')
    if runtime == 'wasm' and identities['go']['node'] != identities['rust']['node']:
        raise ValueError('paired wasm consumers must use the same pinned Node')
    output.mkdir(parents=True)
    (output / 'input.json').write_bytes(archive.input_bytes)
    input_identity = None if input_path is None else probe.identity(input_path)
    if input_identity is not None and input_identity['sha256'] != archive.identity['input_sha256']:
        raise ValueError('input file changed after archive initialization')
    provenance = [probe.identity(archive.inventory / name) for name in ('inventory.json', 'identity.ndjson')] if hasattr(archive, 'inventory') else []
    controllers = [probe.identity(path) for path in
                   (Path(__file__), Path(probe.__file__), Path(stream.__file__),
                    ROOT / 'tools/phase7/budget/snapshot.py', ROOT / 'scripts/s07_benchmark_stats.py',
                    ROOT / 'scripts/s07_benchmark_measure.py', ROOT / 'tools/phase7/bench/capture.py',
                    ROOT / 'tools/phase7/bench/common.py', Path(scenario_input.__file__))]
    helpers = archive_helpers(output, controllers, identities)
    if selection is not None:
        shutil.copyfile(selection['selection_source']['path'], output / 'selection.json')
        shutil.copyfile(selection['descriptor_source']['path'], output / 'descriptor.json')
        selection = dict(selection, selection_archive=probe.identity(output / 'selection.json'),
                         descriptor_archive=probe.identity(output / 'descriptor.json'))
    for name, item in identities.items():
        shutil.copyfile(item['manifest']['path'], output / (name + '-build.json'))
    report = {'schema': 1, 'started_utc': datetime.now(timezone.utc).isoformat(),
              'performance_acceptance': False, 'qualifies': False, 'complete': False, 'status': 'capturing',
              'runtime': runtime, 'measurement_contract': 2,
              'pin': probe.decode(Path(identities['go']['manifest']['path']).read_bytes())['pin'],
              'scope': 'diagnostic frozen-work resource capture; no accepted budget change',
              'elapsed_definition': 'execute request to acknowledgement; RPC included; preparation/checkpoint GC/observation excluded',
              'checkpoint_peak_definition': 'process launch through checkpoint, before observation',
              'retained_definition': 'host ps RSS after checkpoint, while compiler roots are retained',
              'lifecycle_peak_definition': 'process launch through exit, including observation',
              'sampling': {'warmup_pairs': 1, 'initial_pairs': 7, 'maximum_pairs': 14,
                           'extension': 'S07 elapsed uncertainty/MAD only; no sample replacement',
                           'ratios_include_busy_rows': True, 'busy_other_cpu_threshold': BUSY_OTHER_CPU},
              'archive': archive.identity, 'archive_provenance': provenance, 'input_file': input_identity, 'artifacts': identities, 'controllers': controllers,
              'helper_manifest': 'helper-manifest.json', 'scenario_acceptance_binding': selection,
              'startup_metric': None,  # prepare acknowledgement combines startup/module compilation/preparation.
              'eligibility_reasons': sorted(set((['explicit archive has no frozen scenario/mode acceptance binding'] if selection is None else []) +
                                               [reason for item in identities.values() for reason in item['eligibility_reasons']])),
              'warmup': {}, 'pairs': [], 'failures': []}
    path = output / 'measurement.json'
    reference = None
    pending_go = None
    checking_baselines = {}

    def save():
        probe.write(path, report)

    def guard(*, contents=False):
        archive.unchanged()
        if contents:
            verify_contents(archive)
        for item in provenance:
            if probe.identity(item['path']) != item:
                raise ValueError('frozen archive provenance changed during capture')
        if input_identity is not None and probe.identity(input_identity['path']) != input_identity:
            raise ValueError('input file changed during capture')
        for item in identities.values():
            unchanged(item)
        for item in helpers:
            archived = probe.identity(output / item['archived'])
            if any(archived[key] != item[key] for key in ('bytes', 'sha256')):
                raise ValueError('archived helper changed during capture: ' + item['archived'])
        if selection is not None:
            for key in ('selection_source', 'descriptor_source', 'selection_archive', 'descriptor_archive'):
                if probe.identity(selection[key]['path']) != selection[key]:
                    raise ValueError('scenario selection/descriptor changed during capture')
        for item in controllers:
            if probe.identity(item['path']) != item:
                raise ValueError('measurement controller changed during capture: ' + item['path'])
        reject_concurrent_builds()

    def sample(engine, directory, row):
        nonlocal reference, pending_go
        row['record'] = str(directory.relative_to(output) / 'record.json')
        try:
            guard()
            row['other_cpu_before'] = other_cpu()
            busy = row['other_cpu_before']['percent'] > BUSY_OTHER_CPU
            item = identities[engine]
            backend = engine if item['runtime'] == 'native' else engine + '-wasm'
            with launch_environment(item['environment']):
                captured = probe.capture(Path(item['executable']), archive.input, directory, timeout,
                                         backend=backend, preparation=archive.preparation(), protocol_version=2)
            row['raw_files'] = [dict(probe.identity(path), file=path.name)
                                for path in sorted(directory.iterdir()) if path.is_file()]
            row.update(sample_metrics(captured, backend, archive.input['operation']))
            guard(contents=True)
            row['other_cpu_after'] = other_cpu()
            row['busy'] = busy or row['other_cpu_after']['percent'] > BUSY_OTHER_CPU
            observation = captured['observation']
            row['checking_work'] = checking_work(observation, selection, checking_baselines.get(engine))
            checking_baselines.setdefault(engine, int(observation['types']))
            if engine == 'rust' and reference is None:
                reference = observation
                if pending_go is not None:
                    report['warmup']['go']['work'] = work_comparison(reference, pending_go, 'go', selection)
                    pending_go = None
            if reference is None:
                pending_go = observation
            else:
                row['work'] = work_comparison(reference, observation, engine, selection)
            row['types'] = observation['types']  # deliberately not equality: distinct compiler counters
            row['valid'] = True
            row['status'] = 'captured'
            row['qualifies'] = bool(selection is not None and item['production_eligible'] and not row['busy'])
        except Exception as error:
            row.update(valid=False, qualifies=False, status='unavailable', error=str(error))
            raise
        finally:
            save()

    save()
    try:
        report['host'] = host_info(minimum_cpus=1)
        for engine in ('go', 'rust'):
            row = report['warmup'][engine] = {}
            sample(engine, output / ('warmup-' + engine), row)
        target = PAIRS[0]
        while len(report['pairs']) < target:
            index = len(report['pairs'])
            order = ('go', 'rust') if index % 2 == 0 else ('rust', 'go')
            pair = {'index': index, 'order': list(order)}
            report['pairs'].append(pair)
            for engine in order:
                row = pair[engine] = {}
                sample(engine, output / f'pair-{index:02d}-{engine}', row)
            pair['valid'] = True
            pair['busy'] = any(pair[engine]['busy'] for engine in order)
            if len(report['pairs']) == target:
                report['ratios'] = summaries(report['pairs'], runtime)
                if target == PAIRS[0] and report['ratios']['elapsed_rpc_ns']['needs_more']:
                    target = PAIRS[1]
            save()
        guard(contents=True)
        report['complete'] = True
        report['status'] = 'complete_diagnostic'
        report['finished_utc'] = datetime.now(timezone.utc).isoformat()
        report['valid_pairs'] = len(report['pairs'])
        report['idle_pairs'] = sum(not pair['busy'] for pair in report['pairs'])
        report['busy'] = any(row.get('busy') for row in report['warmup'].values()) or any(pair['busy'] for pair in report['pairs'])
        if report['busy']:
            report['eligibility_reasons'].append('busy host rows retained; no filtering or replacement')
        report['eligibility_reasons'].extend(elapsed_qualification_reasons(report['ratios']['elapsed_rpc_ns']))
        report['qualifies'] = not report['eligibility_reasons']
        report['performance_acceptance'] = report['qualifies']
        report['status'] = 'complete' if report['qualifies'] else 'complete_diagnostic'
        report['unavailable_metrics'] = {'retained_bytes': 'native retained heap requires a separate instrumented capture'} if runtime == 'native' else {}
    except Exception as error:
        report['failures'].append(str(error))
        report['status'] = 'unavailable'
    finally:
        report['valid_pairs'] = sum(pair.get('valid', False) for pair in report['pairs'])
        report['idle_pairs'] = sum(pair.get('valid', False) and not pair['busy'] for pair in report['pairs'])
        save()
    return report


def captured_file(directory, relative):
    if not isinstance(relative, str) or Path(relative).is_absolute():
        raise ValueError('capture member must be a relative path')
    path = (directory / relative).resolve(strict=True)
    if not path.is_relative_to(directory.resolve()) or not path.is_file():
        raise ValueError('capture member leaves its archive')
    return path


def verify_saved_artifacts(directory, report):
    helpers = probe.decode(captured_file(directory, report['helper_manifest']).read_bytes())
    archived = {}
    for item in helpers:
        actual = probe.identity(captured_file(directory, item['archived']))
        if any(actual[key] != item[key] for key in ('bytes', 'sha256')) or item['path'] in archived:
            raise ValueError('archived helper identity differs')
        archived[item['path']] = item
    runtime = report['runtime']
    for engine in ('go', 'rust'):
        identity = report['artifacts'][engine]
        build = captured_file(directory, engine + '-build.json')
        actual = probe.identity(build)
        if any(actual[key] != identity['manifest'][key] for key in ('bytes', 'sha256')):
            raise ValueError('archived build manifest changed')
        manifest = probe.decode(build.read_bytes())
        if runtime == 'native' or engine == 'go':
            require_budget_manifest(manifest, engine)
        if engine == 'go':
            if manifest.get('pin') != report['pin']:
                raise ValueError('Go build pin differs from the capture')
            for relative in ('tools/phase7/budget/go/body/operation.go', 'tools/phase7/budget/go/body/stream.go'):
                source = next((item for item in helpers if item['archived'].endswith('/' + relative)), None)
                if source is None or manifest['sources'].get(relative) != source['sha256']:
                    raise ValueError('Go collection implementation is not source-bound')
        if runtime == 'wasm' and engine == 'rust':
            command = manifest.get('command')
            if (manifest.get('mode') != 'checker' or not command_pair(command, '-p', 'tsr_wasm')
                    or '--release' not in command or not command_pair(command, '--features', 'checker')
                    or not manifest.get('sources')):
                raise ValueError('not an ordinary production checker wasm build')
        if runtime == 'native':
            if engine == 'rust':
                if (manifest.get('allocation_instrumented') is not False or manifest.get('allocator') != ALLOCATOR
                        or manifest.get('profile') != {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1}
                        or type(manifest['profile']['codegen_units']) is not int
                        or manifest.get('source_changes_during_build') != []
                        or not command_pair(manifest.get('build_command'), '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml')):
                    raise ValueError('native Rust build is not an uninstrumented ordinary release consumer')
                bound_asset(identity['executable'], {'bytes': manifest['size'], 'sha256': manifest['sha256']})
            else:
                bound_asset(identity['executable'], artifact_entry(manifest, 'budget-go'))
        else:
            environment_key = 'TSR_GO_WASM_ARTIFACT_DIR' if engine == 'go' else 'TSR_WASM_ARTIFACT_DIR'
            names = ('budget-go.wasm', 'wasm_exec.cjs') if engine == 'go' else ('checker_bg.wasm', 'bindings.mjs')
            for name in names:
                bound_asset(Path(identity['environment'][environment_key]) / name, artifact_entry(manifest, name))
        for item in identity['files']:
            if item['path'] == identity['manifest']['path']:
                continue  # The archived manifest was authenticated above.
            if item['path'] in archived:
                if any(archived[item['path']][key] != item[key] for key in ('bytes', 'sha256')):
                    raise ValueError('helper was not bound to the capture identity')
            elif probe.identity(item['path']) != item:
                raise ValueError('captured executable/asset changed')
        if identity['runtime'] != runtime or identity['engine'] != engine:
            raise ValueError('consumer runtime/kind changed')
        env = identity['environment']
        if (engine == 'go' and env.get('GOGC') != '100' or any(env.get(key, 'missing') is not None
                for key in ('GOMEMLIMIT', 'GOEXPERIMENT', 'NODE_OPTIONS'))):
            raise ValueError('captured runtime tuning policy differs')
    for item in report['controllers']:
        if item['path'] not in archived or any(archived[item['path']][key] != item[key] for key in ('bytes', 'sha256')):
            raise ValueError('controller was not archived with its identity')
    if runtime == 'wasm':
        go, rust = (report['artifacts'][engine] for engine in ('go', 'rust'))
        if go['node'] != rust['node'] or go['node_version'] != rust['node_version']:
            raise ValueError('paired wasm runtime is not the same Node')
        package = next((item for item in helpers if item['archived'].endswith('/upstream/package.json')), None)
        if package is None or go['node_version'] != 'v' + probe.decode(captured_file(directory, package['archived']).read_bytes())['volta']['node']:
            raise ValueError('capture did not use pinned Node')
        for item in (go, rust):
            if item['node_arguments'] != ['--expose-gc', '--stack-size=4096'] or item['gc_policy'] != GC_POLICY:
                raise ValueError('capture used unexpected Node flags/collection policy')


def read_capture(directory, scenario, mode, runtime, *, require_qualification=True):
    """Readonly replay: authenticate raw rows, work, and metric sources anew.

    Does not start a child or require the original provisioned project. Frozen
    inventory/content archives and actual retained executable assets must exist.
    Eligibility is independent of threshold misses; native retained heap remains
    unavailable and is therefore still a missing threshold in perf.py check.
    """
    directory = directory.resolve(strict=True)
    report = probe.decode((directory / 'measurement.json').read_bytes())
    if (report.get('complete') is not True or report.get('runtime') != runtime
            or type(report.get('measurement_contract')) is not int or report['measurement_contract'] != 2
            or report.get('schema') != 1):
        raise ValueError('requires a complete contract-2 resource capture of the requested runtime')
    if report['pin'] != probe.decode((ROOT / 'data/upstream.json').read_bytes())['pin']:
        raise ValueError('capture pin differs from repository pin')
    verify_saved_artifacts(directory, report)
    binding = report.get('scenario_acceptance_binding')
    if binding is None:
        raise ValueError('resource capture has no scenario acceptance binding')
    for key, filename in (('selection_archive', 'selection.json'), ('descriptor_archive', 'descriptor.json')):
        actual = probe.identity(captured_file(directory, filename))
        if any(actual[field] != binding[key][field] for field in ('bytes', 'sha256')):
            raise ValueError('archived selection/descriptor changed')
    selection = probe.decode((directory / 'selection.json').read_bytes())
    descriptor = probe.decode((directory / 'descriptor.json').read_bytes())
    if selection['scenario'] != scenario or selection['mode'] != mode:
        raise ValueError('capture scenario/mode differs from requested workload')
    if probe.identity(directory / 'descriptor.json')['sha256'] != selection['descriptor_sha256']:
        raise ValueError('selection does not bind the archived complete descriptor')
    archived_input = captured_file(directory, 'input.json')
    archive = stream.Archive(Path(report['archive']['metadata']), Path(report['archive']['contents']), archived_input)
    for item in report['archive_provenance']:
        if probe.identity(item['path']) != item:
            raise ValueError('frozen archive provenance changed')
    if archive.identity != report['archive'] or binding['input_sha256'] != archive.identity['input_sha256']:
        raise ValueError('frozen input/archive identity differs')
    validate_selection(selection, descriptor, archive.input)
    if probe.identity(common.SCENARIOS / (scenario + '.json'))['sha256'] != selection['descriptor_sha256']:
        raise ValueError('capture descriptor differs from committed scenario definition')
    inventory = probe.decode((archive.inventory / 'inventory.json').read_bytes())
    if inventory['identity_sha256'] != binding['inventory_identity_sha256']:
        raise ValueError('provisioned inventory identity differs')
    binding = dict(binding, selection=selection, descriptor=descriptor)
    pairs = report['pairs']
    if len(pairs) not in PAIRS:
        raise ValueError('resource capture needs seven or fourteen complete pairs')
    reference = None
    pending = None
    checking_baselines = {}
    qualification_reasons = []
    verified = []

    def row(engine, saved, expected_directory):
        nonlocal reference, pending
        expected_record = expected_directory + '/record.json'
        if saved.get('record') != expected_record:
            raise ValueError('raw sample path/order differs')
        record_path = captured_file(directory, expected_record)
        sample_directory = record_path.parent
        raw = saved['raw_files']
        if len({item['file'] for item in raw}) != len(raw) or not {'record.json', 'responses.ndjson.gz', 'stderr'} <= {item['file'] for item in raw}:
            raise ValueError('raw sample archive is missing or duplicated')
        for item in raw:
            if Path(item['file']).name != item['file']:
                raise ValueError('invalid raw sample member name')
            actual = probe.identity(captured_file(sample_directory, item['file']))
            if any(actual[key] != item[key] for key in ('bytes', 'sha256')):
                raise ValueError('raw sample archive changed')
        raw_record = probe.decode(record_path.read_bytes())
        if raw_record.get('observation_record', {}).get('format') != 'rows-v2':
            raise ValueError('resource capture requires authenticated full paged observations')
        raw_record['observation'] = probe.read_observation(sample_directory)
        backend = engine if runtime == 'native' else engine + '-wasm'
        metrics = sample_metrics(raw_record, backend, archive.input['operation'])
        if any(type(saved.get(key)) is not int or saved[key] != metrics[key] for key in METRICS):
            raise ValueError('saved metric differs from its raw source')
        expected_binary = next((item for item in report['artifacts'][engine]['files']
                                if item['path'] == report['artifacts'][engine]['executable']), None)
        if expected_binary is None or raw_record['binary'] != expected_binary:
            raise ValueError('sample did not run the bound executable')
        for key in ('other_cpu_before', 'other_cpu_after'):
            percent = saved[key]['percent']
            if type(percent) not in (int, float) or not math.isfinite(percent) or percent < 0:
                raise ValueError('resource capture contains an invalid host row')
            if percent > BUSY_OTHER_CPU:
                if require_qualification:
                    raise ValueError('resource capture contains a busy host row')
                qualification_reasons.append('busy host row; raw work remains independently validated')
        observation = raw_record['observation']
        checking_work(observation, binding, checking_baselines.get(engine))
        checking_baselines.setdefault(engine, int(observation['types']))
        if engine == 'rust' and reference is None:
            reference = observation
            if pending is not None:
                work_comparison(reference, pending, 'go', binding)
                pending = None
        if reference is None:
            pending = observation
        else:
            work_comparison(reference, observation, engine, binding)
        return metrics

    for engine in ('go', 'rust'):
        row(engine, report['warmup'][engine], 'warmup-' + engine)
    for index, pair in enumerate(pairs):
        order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
        if type(pair.get('index')) is not int or pair['index'] != index or pair.get('order') != order:
            raise ValueError('resource pairs are not in the fixed alternating order')
        measured = {}
        for engine in order:
            measured[engine] = row(engine, pair[engine], f'pair-{index:02d}-{engine}')
        verified.append(measured)
    first = summaries(verified[:7], runtime)['elapsed_rpc_ns']
    if (len(pairs) == 14) != first['needs_more']:
        raise ValueError('capture did not follow the fixed seven-to-fourteen extension rule')
    recomputed = summaries(verified, runtime)
    elapsed_reasons = elapsed_qualification_reasons(recomputed['elapsed_rpc_ns'])
    if elapsed_reasons and require_qualification:
        raise ValueError('resource capture is not qualified: ' + '; '.join(elapsed_reasons))
    qualification_reasons.extend(elapsed_reasons)
    archive.unchanged()
    verify_contents(archive)
    keys = {'elapsed': 'elapsed_rpc_ns', 'peak_rss': 'checkpoint_process_peak_rss_bytes'}
    if runtime == 'wasm':
        keys['retained_rss'] = 'retained_process_rss_bytes'
    return {'ratios': {key: recomputed[field]['ratio'] for key, field in keys.items()},
            'samples': {engine: {key: [pair[engine][field] for pair in verified] for key, field in keys.items()}
                        for engine in ('go', 'rust')},
            'summaries': {key: recomputed[field] for key, field in keys.items()},
            'host': report['host'], 'recorded_at': report['finished_utc'],
            'metadata': {'scenario': scenario, 'mode': mode, 'runtime': runtime, 'pin': report['pin'],
                         'selection': selection, 'archive': archive.identity, 'artifacts': report['artifacts'],
                         'measurement_qualified': not qualification_reasons, 'eligibility_reasons': sorted(set(qualification_reasons)),
                         'elapsed': report['elapsed_definition'], 'peak_rss': report['checkpoint_peak_definition'],
                         'startup': None, 'startup_unavailable': 'no separate init acknowledgement; prepare includes startup/module compilation',
                         'unavailable': {'retained_bytes': 'native retained heap requires separate instrumentation'} if runtime == 'native' else {},
                         'lifecycle_peak_rss_bytes': {engine: [pair[engine]['entire_lifecycle_peak_rss_bytes'] for pair in verified]
                                                      for engine in ('go', 'rust')},
                         'raw_work_differences': [pair[engine].get('work') for pair in pairs for engine in ('go', 'rust')]}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', choices=('native', 'wasm'), required=True)
    for name in ('go', 'rust', 'go-build', 'rust-build', 'inventory', 'contents', 'input', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--selection', type=Path, help='scenario_input selection.json; required for eligible work')
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    try:
        archive = stream.Archive(args.inventory, args.contents, args.input)
        identities = {name: build_identity(args.runtime, name, getattr(args, name), getattr(args, name + '_build'))
                      for name in ('go', 'rust')}
        selection = None if args.selection is None else bind_selection(archive, args.selection)
        report = run(archive, identities, args.output.resolve(), args.timeout, input_path=args.input, selection=selection)
        print(args.output.resolve() / 'measurement.json')
        return 0 if report['complete'] else 2
    except Exception as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
