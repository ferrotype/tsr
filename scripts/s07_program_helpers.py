"""Independent source checks and exact native program-helper test obligations."""
from pathlib import Path
import sys

from s04_common import strict_json_loads
from s06_protocol import canonical, exact_keys, sha256
from s06_utilities import setup, invoke, test_inventory, test_result
from s07_program_compare import ROOT

CHECKS = [
    ['scripts/s07_semver.py'], ['scripts/s07_packagejson.py'],
    ['scripts/s07_option_declarations.py'], ['scripts/s07_program.py', '--check'],
    ['scripts/s07_config.py'],
    ['scripts/s07_module_trace.py', '--check'], ['scripts/s07_config_resolver.py', '--check'],
    ['scripts/s07_config_mappers.py', '--check'], ['scripts/s07_include_reason.py', '--check'],
    ['scripts/s07_verify_options.py', '--check'], ['scripts/s07_path_helpers.py', '--check'],
]


def packagejson_source_report(output, pin):
    from s07_packagejson import QUALIFICATION
    observed = strict_json_loads(output)
    exact_keys(observed, ('schema', 'operation', 'qualification', 'frozen_sha256',
        'fresh_sha256', 'normalized_frozen_sha256', 'normalized_fresh_sha256',
        'raw_equal', 'observations_equal', 'normalized_errors', 'raw_diagnostics',
        'pin', 'requests', 'fresh_artifact', 'manifest_sha256', 'manifest_current'),
        'package JSON source qualification report')
    if (type(observed['schema']) is not int or observed['schema'] != 1
            or observed['operation'] != 'packagejson_source_check' or observed['pin'] != pin
            or observed['qualification'] != QUALIFICATION
            or observed['observations_equal'] is not True or observed['manifest_current'] is not True
            or observed['normalized_frozen_sha256'] != observed['normalized_fresh_sha256']):
        raise ValueError('invalid package JSON source qualification report')
    return observed


def preflight(directory):
    """Build and validate every named obligation before any source capture."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    raw = (ROOT/'data/s07/program-helper-tests.json').read_bytes()
    document = strict_json_loads(raw)
    exact_keys(document, ('schema', 'upstream_pin', 'checks', 'groups'), 'program helper inventory')
    if (type(document['schema']) is not int or document['schema'] != 1
            or document['upstream_pin'] != strict_json_loads((ROOT/'data/upstream.json').read_bytes())['pin']
            or document['checks'] != CHECKS):
        raise ValueError('program helper checker inventory changed')
    groups = document['groups']
    if type(groups) is not list or not groups:
        raise ValueError('missing program helper test groups')
    identities = set()
    for group in groups:
        exact_keys(group, ('package', 'target', 'prefix', 'tests'), 'program helper group')
        identity = (group['package'], group['target'], group['prefix'])
        if identity in identities:
            raise ValueError('duplicate program helper group')
        identities.add(identity)
        if (group['package'] not in {'ts_compiler','ts_module','ts_semver','ts_tspath'}
                or any(type(group[field]) is not str for field in ('target','prefix'))
                or type(group['tests']) is not list or not group['tests']
                or group['tests'] != sorted(set(group['tests']))
                or any(type(name) is not str or not name.startswith(group['prefix']) for name in group['tests'])):
            raise ValueError('invalid program helper test obligation')
    packages = sorted({group['package'] for group in groups})
    args = ['cargo', 'test', '--locked', '--release', '--all-targets', '--no-run', '--message-format=json']
    for package in packages:
        args += ['--package', package]
    output = setup(args, ROOT, directory/'build')
    wanted = {(group['package'], group['target']) for group in groups}
    binaries = {}
    for line in output.splitlines():
        row = strict_json_loads(line)
        if row.get('reason') != 'compiler-artifact' or row.get('profile',{}).get('test') is not True or not row.get('executable'):
            continue
        target = row['target']
        if target['kind'] not in (['lib'], ['test']):
            continue
        package = Path(row['manifest_path']).parent.name
        key = (package, 'lib' if target['kind'] == ['lib'] else target['name'])
        if key not in wanted:
            continue
        if key in binaries:
            raise ValueError('duplicate native program helper test binary')
        binaries[key] = row['executable']
    if set(binaries) != wanted:
        raise ValueError('missing native program helper test binary')
    inventories = {key: setup([binary,'--list','--format=terse'], ROOT, directory/('-'.join(key)+'-inventory')) for key,binary in binaries.items()}
    for group in groups:
        key = (group['package'], group['target'])
        test_inventory(inventories[key], {**group, 'name': '/'.join((*key, group['prefix']))})
    return raw, document, binaries


def measure(directory, prepared=None):
    directory = Path(directory)
    raw, document, binaries = preflight(directory) if prepared is None else prepared
    if (ROOT/'data/s07/program-helper-tests.json').read_bytes() != raw:
        raise ValueError('program helper inventory changed after preflight')
    source_reports = []
    for index, check in enumerate(CHECKS):
        output = setup([sys.executable, *check], ROOT, directory/f'source-{index}')
        if check == ['scripts/s07_packagejson.py']:
            observed = packagejson_source_report(output, document['upstream_pin'])
            source_reports.append({'check': check, 'report': observed})
    results = []
    for index, group in enumerate(document['groups']):
        key = (group['package'], group['target'])
        for ordinal, name in enumerate(group['tests']):
            outcome = invoke([binaries[key],name,'--exact','--test-threads=1','--color=never'], ROOT, directory/f'test-{index}-{ordinal}')
            results.append({'package':key[0], 'target':key[1], 'test':name, 'passed':test_result(outcome,name)})
    report = {'inventory_sha256':sha256(raw), 'source_checks':len(CHECKS),
              'source_reports':source_reports, 'tests':len(results),
              'results':results, 'pass':all(row['passed'] for row in results)}
    (directory/'report.json').write_bytes(canonical(report)+b'\n')
    return report
