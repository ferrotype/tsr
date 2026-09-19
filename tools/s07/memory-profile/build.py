#!/usr/bin/env python3
"""Build additive memory adapters in disposable staged copies of pinned sources."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tomllib

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from s04 import verified_upstream
from s04_common import command
from s07_benchmark import native_environment, go_native_environment, source_fingerprint, rust_native_toolchain, release_configuration, cargo_configuration_paths

STAGE = ROOT / 'target/s07-memory-stage'
OUTPUT = ROOT / 'target/s07-memory-profile'

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def tool_inputs():
    return {str(p.relative_to(ROOT)):sha(p) for p in sorted(TOOLS.rglob('*'))
            if p.is_file() and p.suffix in {'.py','.rs','.go','.toml','.lock'} and 'results' not in p.parts}

def staged_inputs(folder, extensions):
    return {str(p.relative_to(folder)):sha(p) for p in sorted(folder.rglob("*"))
            if p.is_file() and p.suffix in extensions}

def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True)+'\n')

def stage_rust():
    # Fixed disposable destination; never patch real or symlinked source trees.
    if STAGE.is_symlink() or STAGE.resolve() != STAGE:
        raise ValueError('staging path must not be symlinked')
    if STAGE.exists(): shutil.rmtree(STAGE)
    STAGE.mkdir(parents=True)
    for name in ('Cargo.toml','Cargo.lock','rust-toolchain.toml','rustfmt.toml'):
        shutil.copyfile(ROOT/name, STAGE/name)
    for name in ('crates','xtask'):
        shutil.copytree(ROOT/name, STAGE/name, ignore=shutil.ignore_patterns('target','__pycache__'))
    driver = STAGE/'tools/memory-driver'
    shutil.copytree(TOOLS/'rust',driver)
    root_manifest = STAGE/'Cargo.toml'
    root_manifest.write_text(root_manifest.read_text().replace('members = [','members = ["tools/memory-driver", ',1))
    # Profile belongs to the staged root, not a nested package.
    manifest = driver/'Cargo.toml'
    text = manifest.read_text()
    start, end = text.index('[profile.release]'), text.index('[lints.rust]')
    manifest.write_text(text[:start]+text[end:])
    command([sys.executable,str(TOOLS/'rust-census/apply.py'),'--stage',str(STAGE)],cwd=ROOT)
    return driver

def registry_lock():
    root = {(p['name'],p['version'],p.get('source')):p.get('checksum') for p in tomllib.loads((ROOT/'Cargo.lock').read_text())['package'] if p.get('source')}
    staged = [p for p in tomllib.loads((STAGE/'Cargo.lock').read_text())['package'] if p.get('source')]
    extra=[]
    for p in staged:
        key=(p['name'],p['version'],p['source'])
        if key not in root:
            if p['name'] != 'alloc_tracker' or p['version'] != '0.5.25' or p['checksum'] != '70bc5b36f4124124cdeae56ea9527e8db3e90f7ac7382c0b7a07d780163629a4':
                raise ValueError('unreviewed diagnostic dependency '+repr(key))
            extra.append(p)
        elif root[key] != p['checksum']:
            raise ValueError('registry checksum changed')
    return {'root_sha256':sha(ROOT/'Cargo.lock'),'staged_sha256':sha(STAGE/'Cargo.lock'),'additional_diagnostic_dependencies':extra}

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--stage-only',action='store_true')
    ap.add_argument('--rust-only',action='store_true')
    ap.add_argument('--sites',action='store_true')
    args=ap.parse_args()
    if sys.platform!='darwin': raise ValueError('native memory capture supports macOS only')
    OUTPUT.mkdir(parents=True,exist_ok=True)
    before=source_fingerprint()
    inputs=tool_inputs()
    driver=stage_rust()
    if args.sites:
        command([sys.executable,str(TOOLS/'rust-sites/apply.py'),'--stage',str(STAGE)],cwd=ROOT)
        manifest_path=driver/'Cargo.toml'
        manifest_path.write_text(manifest_path.read_text().replace('[dependencies]','[dependencies]\nalloc_tracker = { version = \"=0.5.25\", default-features = false }'))
    if args.stage_only:
        print(driver); return
    env=native_environment()
    env['CARGO_TARGET_DIR']=str(ROOT/'target/s07-memory-build')
    env['DEVELOPER_DIR']='/Applications/Xcode.app/Contents/Developer'
    stable,host=rust_native_toolchain(env)
    config_before={str(p):sha(p) if p.is_file() else None for p in cargo_configuration_paths(env,STAGE)}
    overrides=release_configuration(env,STAGE)
    # Debug symbols are diagnostic only; compiler optimizations remain native.
    overrides += ['--config','profile.release.debug=2','--config','profile.release.split-debuginfo="packed"']
    # Seed root lock and resolve only tool additions, never generate-lockfile.
    command(['cargo','+'+stable,'metadata','--offline','--format-version=1'],cwd=STAGE,env=env)
    registry_lock()
    rust_stage_before=staged_inputs(STAGE,{".rs",".toml",".lock"})
    argv=['cargo','+'+stable,'build','--release','--locked','--manifest-path',str(driver/'Cargo.toml'),'--target',host,'--message-format=json-render-diagnostics',*overrides]
    if args.sites: argv += ['--features','sites']
    messages=command(argv,cwd=STAGE,env=env)
    (OUTPUT/'rust-cargo-messages.ndjson').write_bytes(messages)
    rows=[json.loads(line) for line in messages.splitlines() if line]
    artifacts=[r for r in rows if r.get('reason')=='compiler-artifact' and r.get('target',{}).get('name')=='tsr_memory_profile' and r.get('executable')]
    if len(artifacts)!=1 or rows[-1]!={'reason':'build-finished','success':True}:
        raise ValueError('Cargo did not identify a unique successful adapter build')
    artifact=artifacts[0]
    if (Path(artifact['manifest_path']).resolve()!=driver/'Cargo.toml' or artifact['target']['kind']!=['bin'] or artifact['target']['crate_types']!=['bin']):
        raise ValueError('Cargo artifact does not belong to staged driver')
    if rust_stage_before!=staged_inputs(STAGE,{'.rs','.toml','.lock'}):
        raise ValueError('staged Rust inputs changed during build')
    if artifact.get('features') != (['sites'] if args.sites else []): raise ValueError('wrong adapter features')
    if artifact['profile'] != {'opt_level':'3','debuginfo':2,'debug_assertions':False,'overflow_checks':False,'test':False}:
        raise ValueError('unexpected memory adapter compiler profile')
    cargo_rust=Path(artifact['executable'])
    rust=OUTPUT/('rust-memory-sites' if args.sites else 'rust-memory-native')
    shutil.copy2(cargo_rust,rust)
    shutil.copyfile(STAGE/'Cargo.lock', OUTPUT/'Cargo.lock')
    manifest={'schema':1,'diagnostic_only':True,'source_fingerprint':before,'tool_inputs':inputs,
        'registry_lock':registry_lock(),'staged_rust_inputs':rust_stage_before,'census_manifest':json.loads((STAGE/'census-manifest.json').read_text()),
        'rust':{'toolchain':stable,'host':host,'command':argv,'artifact':artifact,
            'binary':str(rust),'sha256':sha(rust),'allocator':('cap 0.1.2 over alloc_tracker 0.5.25 over mimalloc 0.1.48' if args.sites else 'cap 0.1.2 over mimalloc 0.1.48') + '; libmimalloc-sys from root Cargo.lock'},
        'cargo_configuration':config_before}
    if args.sites:
        manifest['sites_manifest']=json.loads((STAGE/'sites-manifest.json').read_text())
    if not args.rust_only:
        upstream=verified_upstream()
        pin=json.loads((ROOT/'data/upstream.json').read_text())['pin']
        go_stage=ROOT/'target/s07-memory-go-stage'
        if go_stage.is_symlink(): raise ValueError('symlinked Go stage')
        if go_stage.exists(): shutil.rmtree(go_stage)
        go_stage.mkdir()
        archive=command(['git','archive',pin,'tsc/go.mod','tsc/go.sum','tsc/internal'],cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream: stream.extractall(go_stage,filter='data')
        bridge=go_stage/'tsc/internal/s07memoryprofile'; bridge.mkdir()
        shutil.copyfile(TOOLS/'go/main.go',bridge/'main.go')
        shutil.copyfile(TOOLS/'go/ast_bridge.go',go_stage/'tsc/internal/ast/s07_memory_bridge.go')
        go=OUTPUT/'go-memory-profile'
        go_env=go_native_environment(); go_env['GOCACHE']='/private/tmp/ts-rust-s07-go-cache'
        go_argv=['go','build','-trimpath','-mod=readonly','-buildvcs=false','-o',str(go),'./internal/s07memoryprofile']
        go_stage_before=staged_inputs(go_stage,{'.go','.mod','.sum'})
        command(go_argv,cwd=go_stage/'tsc',env=go_env)
        if go_stage_before!=staged_inputs(go_stage,{'.go','.mod','.sum'}): raise ValueError('staged Go inputs changed during build')
        manifest['go']={'staged_inputs':go_stage_before,'upstream_pin':pin,'export_sha256':hashlib.sha256(archive).hexdigest(),
            'command':go_argv,'version':command(['go','version'],cwd=ROOT,env=go_env).decode().strip(),'binary':str(go),'sha256':sha(go)}
    if (before != source_fingerprint() or inputs != tool_inputs() or config_before !=
        {str(p):sha(p) if p.is_file() else None for p in cargo_configuration_paths(env,STAGE)}):
        raise ValueError('compiler or diagnostic sources changed while building; rerun')
    destination=OUTPUT/('build-sites.json' if args.sites else 'build.json')
    write_json(destination,manifest)
    print(json.dumps({'build_manifest':str(destination),'rust_binary':str(rust)}))

if __name__=='__main__': main()
