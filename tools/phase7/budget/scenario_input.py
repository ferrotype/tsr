#!/usr/bin/env python3
"""Select the recorded Phase 7 scenario options for a one-checker consumer.

This supplies no file contents; native discovery uses the separately archived
metadata. No workload or config files are modified.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/phase7/bench'))
import common


def make(name, mode, directory):
    descriptor = common.load(name)
    directory = directory.resolve(strict=True)
    marker = json.loads((directory / common.MARKER).read_bytes())
    descriptor_path = ROOT / 'tools/phase7/scenarios' / f'{name}.json'
    # Provisioning is bound to source/install inputs. Selected compiler options
    # have their own complete descriptor identity below.
    if marker['spec_sha256'] != common.provision_spec(descriptor):
        raise ValueError('provisioned workload does not match source/install inputs')
    common.CACHE = directory.parent
    if common.checkout(descriptor) != directory:
        raise ValueError('expected the provisioned scenario checkout name')
    changed = common.changed_files(descriptor)
    meaningful = [name for name in changed if Path(name).name != '.DS_Store']
    if meaningful:
        raise ValueError('workload changed after provisioning: ' + ', '.join(meaningful[:10]))
    project = directory / descriptor['project']
    config = project if project.is_file() else project / 'tsconfig.json'
    config = config.resolve(strict=True)
    if not config.is_relative_to(directory):
        raise ValueError('scenario config is outside the supplied scope')
    output = f'/phase7-output/{name}/{mode}'
    arguments = [value.replace('@OUT@', output) for value in descriptor['modes'][mode]]
    request = {'current_directory': list(bytes(directory)), 'case_sensitive': True,
               'entries': [], 'configuration': {'config_path': list(bytes(config)),
                   'arguments': [list(value.encode()) for value in arguments]},
               'operation': 'check' if mode == 'check' else 'check_emit'}
    return request, {'scenario': name, 'mode': mode, 'checkers': 1,
                     'descriptor_sha256': hashlib.sha256(descriptor_path.read_bytes()).hexdigest(),
                     'provisioned': marker, 'output_is_in_memory': True,
                     'post_provision_finder_metadata': changed,
                     'finder_metadata_policy': 'retained in filesystem inventory; not compiler source; no file deleted'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('scenario')
    parser.add_argument('mode', choices=['check', 'emit'])
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or 'target' in output.parts or output.is_relative_to(args.directory.resolve()):
        parser.error('output must be new, outside the workload and target')
    request, identity = make(args.scenario, args.mode, args.directory)
    output.mkdir(parents=True)
    (output / 'input.json').write_text(json.dumps(request, separators=(',', ':')) + '\n')
    (output / 'selection.json').write_text(json.dumps(identity, indent=2) + '\n')


if __name__ == '__main__':
    main()
