#!/usr/bin/env python3
"""Build and exercise the Rust child-check analyzer with the tooling nightly."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def runtime_environment(sysroot):
    env = dict(os.environ)
    # rustc_driver also loads LLVM from the host sysroot. An executable rpath
    # does not cover transitive shared-library dependencies on Linux.
    variable = ('PATH' if sys.platform == 'win32' else
                'DYLD_LIBRARY_PATH' if sys.platform == 'darwin' else 'LD_LIBRARY_PATH')
    library = str(Path(sysroot) / ('bin' if sys.platform == 'win32' else 'lib'))
    previous = env.get(variable)
    env[variable] = library + (os.pathsep + previous if previous else '')
    return env


def build():
    nightly = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['nightly']
    rustc = ['rustup', 'run', nightly, 'rustc']
    identity = subprocess.check_output([*rustc, '-Vv'])
    digest = hashlib.sha256((HERE / 'main.rs').read_bytes() + identity).hexdigest()[:16]
    directory = ROOT / 'target/phase7/checkchildren'
    directory.mkdir(parents=True, exist_ok=True)
    executable = directory / ('checkchildren-' + digest)
    sysroot = subprocess.check_output([*rustc, '--print', 'sysroot'], text=True).strip()
    if not executable.exists():
        subprocess.run([*rustc, str(HERE / 'main.rs'), '--crate-name', 'tsr_checkchildren',
                        '--edition=2021', '-C', 'rpath', '-L', str(Path(sysroot) / 'lib'),
                        '-D', 'warnings', '-o', str(executable)], cwd=ROOT, check=True)
    return nightly, executable, runtime_environment(sysroot)


def fixtures(executable, nightly, runtime_env):
    source = (HERE / 'fixtures/returns.rs').read_text()
    expected = sorted(re.findall(r'// expect: (\w+)', source))
    if not expected:
        raise ValueError('fixture has no planted omissions')
    with tempfile.TemporaryDirectory(prefix='checkchildren-') as temporary:
        directory = Path(temporary)

        def run(text):
            path = directory / 'case.rs'
            path.write_text(text)
            result = subprocess.run([str(executable), '--crate-name', 'checkchildren_fixture',
                                     str(path), '--edition=2021', '--emit=metadata',
                                     '--out-dir', str(directory)], cwd=ROOT, env=runtime_env,
                                    capture_output=True, text=True)
            findings = sorted(re.findall(r'checkchildren: Checker::(\w+) returns before', result.stderr))
            return result, findings

        result, findings = run(source)
        if result.returncode != 1 or findings != expected or 'early returns' not in result.stderr:
            raise ValueError(f'planted omissions differ: expected {expected}, got {findings}\n{result.stderr}')
        # Remove the planted returns. A let-else must still diverge, so use
        # panic for that synthetic control; this is not a production repair.
        fixed = re.sub(r'(let [^\n]* else \{\s*)return;(\s*\}; // expect:)',
                       r'\1panic!();\2', source)
        fixed = re.sub(r'\breturn(?:\s+[^;]+)?;(\s*\}[^\n]*// expect:)', r'();\1', fixed)
        result, findings = run(fixed)
        if result.returncode != 0 or findings:
            raise ValueError('fixed fixture rejected:\n' + result.stderr)
        if '1 reviewed allowances' not in result.stderr:
            raise ValueError('allowance was not reported')
        result, _ = run(fixed.replace(
            '// checkchildren-allow: deliberate fixture branch; other return remains checked',
            '// checkchildren-allow:'))
        if result.returncode != 1 or 'empty allowance reason' not in result.stderr:
            raise ValueError('empty allowance reason was accepted:\n' + result.stderr)
        result, _ = run(source.replace('check_source_element_worker', 'unrelated_worker'))
        if result.returncode != 1 or 'no dispatch workers found' not in result.stderr:
            raise ValueError('empty worker inventory was accepted:\n' + result.stderr)
        # Rustc JSON diagnostics are retained by Cargo. Check that the summary
        # remains observable when the wrapper is skipped for a Fresh artifact.
        (directory / 'Cargo.toml').write_text(
            '[package]\nname = "checkchildren_fixture"\nversion = "0.0.0"\nedition = "2021"\n'
            '[lib]\npath = "case.rs"\n[workspace]\n')
        (directory / 'case.rs').write_text(fixed)
        env = dict(runtime_env, RUSTC_WORKSPACE_WRAPPER=str(executable),
                   CARGO_TARGET_DIR=str(directory / 'target'))
        command = ['cargo', '+' + nightly, 'check', '--offline', '--manifest-path',
                   str(directory / 'Cargo.toml'), '-v']
        for cached in (False, True):
            result = subprocess.run(command, cwd=directory, env=env, capture_output=True, text=True)
            if (result.returncode != 0
                    or not re.search(r'note: checkchildren: 1 workers, \d+ dispatched methods, 0 early returns', result.stderr)
                    or (cached and 'Fresh checkchildren_fixture' not in result.stderr)):
                raise ValueError(f'Cargo summary cache control failed (cached={cached}):\n' + result.stderr)
    print(f'checkchildren fixtures: {len(expected)} planted returns, repaired control, '
          'empty-roster refusal, precise allowance, empty-reason refusal, Cargo summary replay pass')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['fixtures', 'check'])
    parser.add_argument('--output', type=Path, help='new retained log outside target for check')
    args = parser.parse_args()
    if args.action == 'check' and (args.output is None or args.output.exists()
                                   or 'target' in args.output.resolve().parts):
        parser.error('check requires a new --output log outside target')
    nightly, executable, runtime_env = build()
    fixtures(executable, nightly, runtime_env)
    if args.action == 'fixtures':
        return 0
    env = dict(runtime_env, RUSTC_WORKSPACE_WRAPPER=str(executable),
               CARGO_TARGET_DIR=str(ROOT / 'target/phase7/checkchildren-cargo'))
    env.setdefault('CARGO_BUILD_JOBS', '2')
    command = ['cargo', '+' + nightly, 'check', '--locked', '-p', 'tsr_checker']
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as log:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
    text = args.output.read_text()
    print(text)
    if result.returncode != 0:
        return result.returncode
    if not re.search(r'checkchildren: 2 workers, \d+ dispatched methods, 0 early returns', text):
        # Cargo replays the rustc diagnostic on a successful cached compile.
        # A missing summary is never evidence of a clean lint run.
        print('No checker lint summary; refusing an unobserved lint result.')
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
