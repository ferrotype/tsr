#!/usr/bin/env python3
"""Build isolated wasm instances with a version-matched wasm-bindgen CLI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from s04_ownership import instrumentation_environment
from s10_corpus import sources

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build_environment():
    env = instrumentation_environment(os.environ, ROOT)
    for key in list(env):
        if key.startswith('CARGO_PROFILE_') or key in ('CARGO_BUILD_TARGET', 'CARGO_BUILD_TARGET_DIR'):
            env.pop(key)
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target')
    expected = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    actual = subprocess.check_output(['rustc', '--version'], cwd=ROOT, env=env, text=True).split()[1]
    if actual != expected:
        raise ValueError(f'S10 requires Rust {expected}, found {actual}')
    return env


def build(bindgen, *, checker=False, corpus=False, wasm_opt=None):
    before = sources()
    pin = json.loads((ROOT / 'tools/s10/toolchains.json').read_text())
    version = subprocess.check_output([str(bindgen), '--version'], text=True).strip()
    if version != 'wasm-bindgen ' + pin['wasm_bindgen']:
        raise ValueError('wasm-bindgen CLI must match the pinned crate: ' + version)
    mode = 'corpus' if corpus else 'checker' if checker else 'parser'
    optimizer_version = None
    if mode == 'parser':
        if wasm_opt is None:
            raise ValueError('parser release requires --wasm-opt (scripts/s10_toolchain.py provisions it)')
        optimizer_version = subprocess.check_output([str(wasm_opt), '--version'], text=True).strip()
        expected = f"wasm-opt version {pin['wasm_opt_version']} (version_{pin['wasm_opt_version']})"
        if optimizer_version != expected:
            raise ValueError('wasm-opt differs from the pinned release')
    command = ['cargo', 'build', '--locked', '--release', '--target', pin['wasm_target'], '-p', 'ts_wasm']
    if checker or corpus:
        command += ['--features', 'corpus' if corpus else 'checker']
    env = dict(build_environment(), CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS=pin['wasm_rustflags'])
    # Avoid a caller's global flags silently taking precedence over target flags.
    for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
        env.pop(key, None)
    subprocess.run(command, cwd=ROOT, env=env, check=True)
    output = ROOT / 'target/s10' / mode
    output.mkdir(parents=True, exist_ok=True)
    original = ROOT / 'target' / pin['wasm_target'] / 'release/ts_wasm.wasm'
    subprocess.run([str(bindgen), '--target', 'no-modules', '--remove-name-section', '--remove-producers-section', '--out-dir', str(output),
                    '--out-name', mode, str(original)], cwd=ROOT, check=True)
    post_link = None
    post_input = output / 'before-opt.wasm'
    if mode == 'parser':
        binary = output / 'parser_bg.wasm'
        post_input.write_bytes(binary.read_bytes())
        optimize = [str(wasm_opt), *pin['parser_post_link_flags'], str(post_input), '-o', str(binary)]
        subprocess.run(optimize, cwd=ROOT, check=True)
        post_link = {'command': optimize, 'version': optimizer_version, 'tool_sha256': digest(wasm_opt)}
    # No rewriting of ABI glue: put the entire generated IIFE in a factory so
    # import caching cannot retain every instance or share poisoned globals.
    glue = (output / (mode + '.js')).read_text()
    if not glue.startswith('let wasm_bindgen = (function(exports) {'):
        raise ValueError('unexpected pinned wasm-bindgen no-modules output')
    factory = output / 'bindings.mjs'
    factory.write_text('export function createBindings() {\n' + glue + '\nreturn wasm_bindgen;\n}\n')
    # Each feature build overwrites Cargo's common output. Preserve its input
    # here so building checker/corpus cannot invalidate a parser build record.
    preserved = output / 'input.wasm'
    preserved.write_bytes(original.read_bytes())
    files = [preserved, output / (mode + '_bg.wasm'), output / (mode + '.js'), factory]
    if post_link:
        files.append(post_input)
    record = {
        'post_link': post_link,
        'sources': before,
        'mode': mode, 'command': command, 'rustflags': pin['wasm_rustflags'],
        'rustc': subprocess.check_output(['rustc', '--version'], env=build_environment(), text=True).strip(),
        'wasm_bindgen': version, 'wasm_bindgen_sha256': digest(bindgen),
        'inputs': {str(p.relative_to(ROOT)): digest(p) for p in
                   [ROOT / 'Cargo.lock', ROOT / 'tools/s10/toolchains.json', Path(__file__)]},
        'artifacts': {str(p.relative_to(ROOT)): {'sha256': digest(p), 'bytes': p.stat().st_size} for p in files},
    }
    if sources() != before:
        raise ValueError('source changed while building wasm; repeat the build before capture')
    (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    return output


def build_consumer():
    before = sources()
    command = ['cargo', 'build', '--locked', '--release', '--manifest-path',
               'tools/s10/rust-consumer/Cargo.toml', '--target-dir', 'target']
    subprocess.run(command, cwd=ROOT, env=build_environment(), check=True)
    if sources() != before:
        raise ValueError('source changed while building the external consumer')
    binary = ROOT / 'target/release/s10_rust_consumer'
    record = {'sources': before, 'command': command, 'binary_sha256': digest(binary),
              'rustc': subprocess.check_output(['rustc', '--version'], env=build_environment(), text=True).strip()}
    output = ROOT / 'target/s10/rust-consumer-build.json'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(record, indent=2) + '\n')
    return output


def build_node():
    before = sources()
    command = ['cargo', 'build', '--locked', '--release', '-p', 'ts_node']
    subprocess.run(command, cwd=ROOT, env=build_environment(), check=True)
    if sources() != before:
        raise ValueError('source changed while building Node adapter')
    name = 'libts_node.dylib' if sys.platform == 'darwin' else 'ts_node.dll' if sys.platform == 'win32' else 'libts_node.so'
    original = ROOT / 'target/release' / name
    directory = ROOT / 'target/s10/node'
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / 'ts_node.node'
    binary.write_bytes(original.read_bytes())
    record = {'sources': before, 'command': command, 'binary_sha256': digest(binary),
              'rustc': subprocess.check_output(['rustc', '--version'], env=build_environment(), text=True).strip()}
    (directory / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    return directory


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasm-bindgen', type=Path)
    parser.add_argument('--wasm-opt', type=Path)
    parser.add_argument('--consumer', action='store_true')
    parser.add_argument('--node', action='store_true')
    parser.add_argument('--checker', action='store_true')
    parser.add_argument('--corpus', action='store_true')
    args = parser.parse_args()
    if args.node:
        print(build_node())
    elif args.consumer:
        print(build_consumer())
    else:
        if args.wasm_bindgen is None:
            parser.error('wasm builds require --wasm-bindgen')
        print(build(args.wasm_bindgen.resolve(), checker=args.checker, corpus=args.corpus,
                    wasm_opt=args.wasm_opt.resolve() if args.wasm_opt else None))
