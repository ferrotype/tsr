"""Capture/replay bounded E2 comparator and native-stack obligations."""
import lzma
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

import s08_p4 as p4
from s08_oracle import ROOT, digest
from s08_p3_comparators import compare as compare_order
from s06_utilities import test_result
from s04 import same_json_value

MANIFEST = ROOT / 'data/s08/e2-obligations.json'


def inputs():
    manifest = p4.read(MANIFEST)
    if manifest['version'] != 1 or manifest['pin'] != p4.read(ROOT / 'data/upstream.json')['pin']:
        raise ValueError('E2 obligation version/pin changed')
    # Read one named member; no tar paths are extracted into the workspace.
    with tarfile.open(ROOT / 'tools/s08/results/p0-contracts/native.tar.xz') as archive:
        requests = archive.extractfile('p0-final-verification/supplemental/requests.json').read()
        native = archive.extractfile('p0-final-verification/supplemental/observations.json').read()
    frozen = lzma.decompress((ROOT / 'data/s08/supplemental-observations.json.xz').read_bytes())
    if not same_json_value(p4.strict_json_loads(native), p4.strict_json_loads(frozen)):
        raise ValueError('archived native comparator/deep observations differ from the frozen authority')
    order = p4.read(ROOT / 'tools/s08/p3c/order-inputs.json')
    if digest(requests) != order['request_sha256'] or digest(native) != order['native_sha256']:
        raise ValueError('frozen comparator/deep native inputs changed')
    return manifest, requests, native, order


def invoke(directory, name, command, *, timeout=300):
    prefix = directory / name
    try:
        result = subprocess.run(list(map(str, command)), cwd=ROOT, capture_output=True, timeout=timeout, check=False)
        code = result.returncode
        stdout, stderr = result.stdout, result.stderr
    except subprocess.TimeoutExpired as error:
        code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
    prefix.with_suffix('.stdout').write_bytes(stdout)
    prefix.with_suffix('.stderr').write_bytes(stderr)
    p4.write_new(prefix.with_suffix('.command.json'), {'command': list(map(str, command)), 'returncode': code})
    return subprocess.CompletedProcess(command, code, stdout, stderr)


def listed(stdout, names):
    found = re.findall(r'^(\S+): test$', stdout.decode(), re.MULTILINE)
    if not names or len(set(names)) != len(names) or any(found.count(n) != 1 for n in names):
        raise ValueError('E2 named recursion test missing or duplicated')


def capture(directory, source_fn):
    manifest, requests, native, order = inputs()
    directory.mkdir(parents=True, exist_ok=False)
    before = source_fn()
    (directory / 'requests.json').write_bytes(requests)
    (directory / 'native.json').write_bytes(native)
    p4.write_new(directory / 'order-inputs.json', order)
    # Validate actual test inventory before executing any corpus or runtime probe.
    test_runs = []
    for profile in manifest['profiles']:
        for index, group in enumerate(manifest['tests']):
            name = f'{profile}-{index}'
            command = ['cargo', 'test', '--locked', '-p', group['package'], '--all-features', '--no-run', '--message-format=json']
            command += ['--lib'] if group['target'] == 'lib' else ['--test', group['target']]
            if profile == 'release': command += ['--release']
            build = invoke(directory, name + '-build', command)
            if build.returncode != 0: raise ValueError('E2 test build failed: ' + name)
            binaries = [event['executable'] for event in map(p4.strict_json_loads, build.stdout.splitlines())
                        if event.get('reason') == 'compiler-artifact' and event.get('executable') and event['profile']['test']
                        and event['target']['name'] == (group['package'] if group['target'] == 'lib' else group['target'])]
            if len(binaries) != 1: raise ValueError('ambiguous E2 test executable')
            binary = directory / (name + '-executable')
            shutil.copy2(binaries[0], binary)
            inventory = invoke(directory, name + '-inventory', [binary, '--list', '--format=terse'])
            if inventory.returncode != 0: raise ValueError('E2 test inventory failed')
            listed(inventory.stdout, group['names'])
            test_runs.append((name, binary, group['names']))
    for name, binary, names in test_runs:
        for index, test in enumerate(names):
            invoke(directory, f'{name}-test-{index}', [binary, test, '--exact', '--test-threads=1', '--color=never'])
    for example, arguments in (
        ('p3_comparators', [directory / 'requests.json', directory / 'order-inputs.json', directory / 'comparators.json']),
        ('p3_relations', [directory / 'requests.json', directory / 'recursion.json', '--deep-runtime']),
    ):
        build = p4.build(directory / example, example=example, source_fn=source_fn, optimize=True,
                         features=('relation-probe', 'ts_checker/storage-pilot'))
        binary = directory / (example + '-executable')
        shutil.copy2(build['binary'], binary)
        invoke(directory, example + '-run', [binary, *arguments])
    if before != source_fn(): raise ValueError('sources changed during E2 obligations')
    artifacts = {str(p.relative_to(directory)): digest(p.read_bytes()) for p in sorted(directory.rglob('*')) if p.is_file()}
    p4.write_new(directory / 'capture.json', {'version': 1, 'sources': before, 'artifacts': artifacts})
    result = replay(directory, source_fn)
    p4.atomic(directory / 'report.json', result)
    return result


def replay(directory, source_fn):
    manifest, requests, native, order = inputs()
    capture = p4.read(directory / 'capture.json')
    if capture['version'] != 1 or capture['sources'] != source_fn():
        raise ValueError('stale E2 obligations; recapture on current sources')
    for name, expected in capture['artifacts'].items():
        if Path(name).is_absolute() or '..' in Path(name).parts or digest((directory / name).read_bytes()) != expected:
            raise ValueError('E2 obligation raw artifact changed: ' + name)
    for name, value in (('requests.json', requests), ('native.json', native)):
        if (directory / name).read_bytes() != value: raise ValueError('E2 obligation input changed')
    if p4.read(directory / 'order-inputs.json') != order: raise ValueError('ordering inputs changed')

    def result(name):
        command = p4.read(directory / (name + '.command.json'))
        for suffix in ('.command.json', '.stdout', '.stderr'):
            if name + suffix not in capture['artifacts']: raise ValueError('unbound obligation output')
        return subprocess.CompletedProcess(command['command'], command['returncode'],
            (directory / (name + '.stdout')).read_bytes(), (directory / (name + '.stderr')).read_bytes())

    outcomes = []
    for profile in manifest['profiles']:
        for index, group in enumerate(manifest['tests']):
            name = f'{profile}-{index}'
            binary = name + '-executable'
            if binary not in capture['artifacts']: raise ValueError('missing captured test binary')
            inventory = result(name + '-inventory')
            if inventory.returncode != 0: raise ValueError('test listing failed')
            listed(inventory.stdout, group['names'])
            for i, test in enumerate(group['names']):
                observed = result(f'{name}-test-{i}')
                if observed.args[1:] != [test, '--exact', '--test-threads=1', '--color=never']:
                    raise ValueError('test command no longer names exact obligation')
                try:
                    passed, reason = test_result(observed, test), None
                except ValueError as error:
                    passed, reason = False, str(error)
                outcomes.append({'profile': profile, 'test': test, 'pass': passed, 'reason': reason})

    comparator = {'pass': False}
    if result('p3_comparators-run').returncode == 0:
        if 'comparators.json' not in capture['artifacts']: raise ValueError('missing comparator output')
        try:
            checked = compare_order(requests, native, order, p4.read(directory / 'comparators.json'))
            if any(checked[k] != v for k, v in manifest['comparator_counts'].items()):
                raise ValueError('comparator coverage differs from reviewed inventory')
            comparator.update(checked, **{'pass': True})
        except (ValueError, KeyError, TypeError) as error:
            comparator['reason'] = str(error)
    deep = {'pass': False}
    if result('p3_relations-run').returncode == 0:
        if 'recursion.json' not in capture['artifacts']: raise ValueError('missing recursion output')
        actual = p4.read(directory / 'recursion.json')
        expected = [r for r in p4.strict_json_loads(native)['rows'] if r['id'] in manifest['deep_cases']]
        if (actual.get('request_sha256') != digest(requests) or actual.get('deep_runtime') is not True
                or actual.get('stack_bytes') != manifest['stack_bytes']
                or [r['id'] for r in actual['rows']] != manifest['deep_cases']
                or [r['id'] for r in expected] != manifest['deep_cases']):
            raise ValueError('deep fixture identity, stack or native request changed')
        differences = [r['id'] for r, a in zip(expected, actual['rows'], strict=True) if not same_json_value(r, a)]
        deep.update({'pass': not differences, 'differences': differences})
    return {'comparators': comparator, 'deep_recursion': deep, 'tests': outcomes,
            'metrics': {'comparators': comparator['pass'], 'recursion_fixtures': deep['pass'] and all(r['pass'] for r in outcomes)}}
