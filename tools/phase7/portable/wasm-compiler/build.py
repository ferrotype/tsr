#!/usr/bin/env python3
"""Build the test-only public embedding wasm consumer, never a performance artifact."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import shutil
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'scripts'))
from s10_build import build_environment  # noqa: E402

HERE = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prefetch_registry():
    # A host/wasm build fetches only its active target dependencies. Unfiltered
    # metadata resolves the entire locked graph, including Windows packages.
    # Fetch that graph before the deliberately offline freshness snapshots.
    subprocess.run(['cargo', 'fetch', '--locked', '--manifest-path', str(HERE / 'Cargo.toml')],
                   cwd=ROOT, check=True)


def source_snapshot():
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--format-version=1',
        '--manifest-path', str(HERE / 'Cargo.toml')], cwd=ROOT, text=True))
    files = {HERE / 'Cargo.lock', HERE / 'Cargo.toml', Path(__file__), ROOT / 'Cargo.toml',
             ROOT / 'rust-toolchain.toml', ROOT / 'tools/s10/toolchains.json', ROOT / 'upstream/package.json',
             ROOT / 'scripts/s10_build.py', ROOT / 'scripts/s04_ownership.py',
             HERE.parent / 'compiler-wasm.mjs', HERE.parent / 'wasm-consumer'}
    for package in metadata['packages']:
        if package['source'] is not None:
            continue  # Registry versions/checksums are bound by this crate's lock.
        directory = Path(package['manifest_path']).parent
        files.add(directory / 'Cargo.toml')
        for child in ('src', 'bundled'):
            files.update(path for path in (directory / child).rglob('*') if path.is_file())
        if (directory / 'build.rs').is_file():
            files.add(directory / 'build.rs')
    files.update(path for path in (ROOT / '.cargo').rglob('*') if path.is_file())
    files.update(HERE.parent / name for name in [
        'protocol.rs', 'compiler_consumer.rs', 'compiler_protocol.rs', 'compiler_codec.rs',
        'mapper_codec.rs', 'assertions.rs'])
    files.update(HERE.parent.glob('walker/**/*.rs'))
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasm-bindgen', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if any(part.casefold() == 'target' for part in output.parts):
        parser.error('retained test artifacts must be outside every target directory')
    if output.exists():
        parser.error('output must be a new directory')
    pin = json.loads((ROOT / 'tools/s10/toolchains.json').read_text())
    version = subprocess.check_output([str(args.wasm_bindgen), '--version'], text=True).strip()
    if version != 'wasm-bindgen ' + pin['wasm_bindgen']:
        parser.error('wasm-bindgen CLI differs from the repository pin')
    node = subprocess.check_output(['node', '--version'], text=True).strip()
    expected_node = 'v' + json.loads((ROOT / 'upstream/package.json').read_text())['volta']['node']
    if node != expected_node:
        parser.error(f'Node differs from the repository pin: {node}, expected {expected_node}')
    prefetch_registry()
    before = source_snapshot()
    env = dict(build_environment(), CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS=pin['wasm_rustflags'])
    for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
        env.pop(key, None)
    command = ['cargo', 'build', '--locked', '--release', '--manifest-path', str(HERE / 'Cargo.toml'),
               '--target', pin['wasm_target'], '--message-format=json-render-diagnostics']
    completed = subprocess.run(command, cwd=ROOT, env=env, check=True, stdout=subprocess.PIPE, text=True)
    artifacts = [value for line in completed.stdout.splitlines()
                 if (value := json.loads(line)).get('reason') == 'compiler-artifact'
                 and value['target']['name'] == 'phase7_wasm_compiler'
                 and 'cdylib' in value['target']['crate_types']]
    wasm = [Path(name) for artifact in artifacts for name in artifact['filenames'] if name.endswith('.wasm')]
    if len(wasm) != 1:
        raise ValueError('Cargo did not report exactly one compiler wasm artifact')
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([str(args.wasm_bindgen), '--target', 'no-modules', '--remove-name-section',
                    '--remove-producers-section', '--out-dir', str(output), '--out-name', 'compiler',
                    str(wasm[0])], cwd=ROOT, check=True)
    glue = (output / 'compiler.js').read_text()
    if not glue.startswith('let wasm_bindgen = (function(exports) {'):
        raise ValueError('unexpected pinned wasm-bindgen no-modules output')
    (output / 'bindings.mjs').write_text('export function createBindings() {\n' + glue + '\nreturn wasm_bindgen;\n}\n')
    # Retain the exact transport as well as generated ABI assets. Detached
    # replay must not need target output or a later repository JS revision.
    for name in ('compiler-wasm.mjs', 'wasm-consumer'):
        shutil.copy2(HERE.parent / name, output / name)
    imports = json.loads(subprocess.check_output([
        'node', '--input-type=module', '-e',
        'import{readFileSync}from"node:fs";console.log(JSON.stringify(WebAssembly.Module.imports(new WebAssembly.Module(readFileSync(process.argv[1])))))',
        str(output / 'compiler_bg.wasm')], text=True))
    after = source_snapshot()
    if before != after:
        raise ValueError('source changed during build; retain failure and repeat before acceptance')
    record = {'version': 1, 'mode': 'phase7-test-compiler', 'package': 'phase7_wasm_compiler',
              'performance_eligible': False, 'command': command, 'sources': before, 'imports': imports,
              'rustflags': pin['wasm_rustflags'], 'node': node, 'node_flags': pin['node_flags'],
              'rustc': subprocess.check_output(['rustc', '--version'], env=env, text=True).strip(),
              'wasm_bindgen': version, 'wasm_bindgen_sha256': digest(args.wasm_bindgen),
              'artifacts': {name: {'sha256': digest(output / name), 'bytes': (output / name).stat().st_size}
                            for name in ['compiler_bg.wasm', 'compiler.js', 'bindings.mjs', 'compiler-wasm.mjs', 'wasm-consumer']}}
    (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    print(output)


if __name__ == '__main__':
    main()
