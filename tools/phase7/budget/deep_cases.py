#!/usr/bin/env python3
"""Named full-pipeline native stress inputs for the public budget protocol.

Use probe.py for exact Go/Rust observations. Depths are explicit tested bounds,
not claims of unlimited recursion; the probe runs each compiler in a process.
The production wasm route must be tested separately with its engine settings.
"""
import argparse
import json
from pathlib import Path


def cases():
    return {
        'left-addition-5000': ('main.ts', 'export const result = ' + ' + '.join(['1'] * 5001) + ';\n', 5000),
        'right-exponentiation-5000': ('main.ts', 'export const result = ' + ' ** '.join(['1'] * 5001) + ';\n', 5000),
        'left-addition-100000': ('main.ts', 'export const result = ' + ' + '.join(['1'] * 100001) + ';\n', 100000),
        'right-exponentiation-100000': ('main.ts', 'export const result = ' + ' ** '.join(['1'] * 100001) + ';\n', 100000),
        'parentheses-5000': ('main.ts', 'export const result = ' + '(' * 5000 + '1' + ')' * 5000 + ';\n', 5000),
        'jsx-600': ('main.tsx', 'declare global { namespace JSX { interface IntrinsicElements { x: {}; } } }\n'
                    + 'export const result = ' + '<x>' * 600 + '</x>' * 600 + ';\n', 600),
        'conditional-types-600': ('main.ts', 'export type Conditional<T> = '
                    + '(T extends number ? number : ' * 600 + 'boolean' + ')' * 600
                    + ';\nexport let result: Conditional<string>;\n', 600),
        'nested-statements-1600': ('main.ts', 'export let result = 0;\n' + 'if (true) {' * 1600
                                 + 'result = 1;' + '}' * 1600 + '\n', 1600),
    }


def request(name):
    filename, text, _ = cases()[name]
    config = {'compilerOptions': {'strict': True, 'target': 'esnext', 'module': 'esnext',
                                 'jsx': 'preserve', 'sourceMap': True, 'outDir': 'out'},
              'files': [filename]}
    return {'current_directory': list(b'/project'), 'case_sensitive': True,
            'entries': [{'kind': 'physical_file', 'path': list(b'/project/tsconfig.json'),
                         'bytes': list(json.dumps(config).encode())},
                        {'kind': 'physical_file', 'path': list(('/project/' + filename).encode()),
                         'bytes': list(text.encode())}],
            'configuration': {'config_path': list(b'/project/tsconfig.json'), 'arguments': []},
            'operation': 'check_emit'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case', choices=cases())
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    path = args.output.resolve()
    if path.exists() or 'target' in path.parts:
        parser.error('output must be a new file outside target')
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('x') as output:
        json.dump(request(args.case), output, separators=(',', ':'))
        output.write('\n')


if __name__ == '__main__':
    main()
