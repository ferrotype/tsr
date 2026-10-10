"""The Phase 7 scenarios and one compiler run (docs/PHASE7-plan.md, R0 items 1-2).

A scenario is a committed descriptor, tools/phase7/scenarios/<name>.json:

- `source`: a GitHub repository and the commit whose archive is compiled;
- `install`: shell commands run once, in a pinned Node container that sees
  only the scenario's directory, to install its dependencies (the
  TypeScript-benchmarking repository sandboxes the same installs);
- `project`: the argument of `-p`, relative to the checkout;
- `modes`: the arguments of full checking (`check`) and of checking plus emit
  (`emit`, whose `@OUT@` is a fresh directory outside the checkout);
- `work`: per mode, what the pinned tsgo (the oracle) compiles and produces at
  four checkers: the listed files and their bytes, the resolved options, the
  exit status, the diagnostics and file list it prints, its file and type
  counts, and the files it emits.

`run` executes one compiler process with `--pretty false --listFiles
--extendedDiagnostics` and the requested `--checkers`, timing it from launch
to exit and taking its peak RSS from its own rusage. `observe` reduces the
run to comparable facts and `problems` lists every way they differ from the
frozen work: a run whose output, file set, emitted bytes or checking work
differ from the oracle's contributes no sample.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
SCENARIOS = ROOT / 'tools/phase7/scenarios'
CACHE = ROOT.parent / '.ts-rust-workloads' / 'phase7.noindex'
BINARIES = ROOT / 'target/phase7/bin'
NAMES = ('vscode', 'self-compiler', 'mui-docs', 'xstate', 'bluesky')
MODES = ('check', 'emit')
CHECKERS = (2, 4, 8)
ORACLE_CHECKERS = 4
ROOT_TOKEN = '@SCENARIO@'
OUT_TOKEN = '@OUT@'
TIMEOUT_SECONDS = 1800
MARKER = '.phase7-provisioned.json'
# A run that checked fewer types than this share of the oracle's skipped
# checking work (the oracle's count is at four checkers; two checkers create
# fewer duplicate types, so the floor is loose).
TYPES_FLOOR = 0.5
STATISTIC = re.compile(r'^([A-Z][A-Za-z ]*?):\s+(\S+)$')
DIAGNOSTIC = re.compile(r'^\S.*\(\d+,\d+\): (?:error|warning|message) TS\d+: |^(?:error|warning|message) TS\d+: ')
SHA = re.compile(r'^[0-9a-f]{64}$')
COMMIT = re.compile(r'^[0-9a-f]{40}$')
IMAGE = re.compile(r'^[a-z0-9._/-]+:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}$')
REPOSITORY = re.compile(r'^https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)$')
WORK_FIELDS = {'checkers', 'exit', 'files', 'listed', 'inputs_sha256', 'options_sha256', 'output_sha256',
               'diagnostics', 'types'}
EMIT_FIELDS = {'emitted', 'emitted_sha256', 'oracle_runs'}
# `emitted` and `emitted_sha256` are the union of the oracle's runs. The pin
# loads files in parallel and, on mui-docs, emits a varying subset of the
# workspace sources it reaches through both node_modules symlinks and their
# real paths; such an oracle also records how many files every one of its runs
# emitted. Its own samples may then omit files, never change one.
EMIT_OPTIONAL = {'emitted_in_every_run'}
ORACLE_EMIT_RUNS = (3, 12)
# The most union files one run of a varying oracle may omit (the pin's runs on
# mui-docs omitted up to 27 of 13,399).
ORACLE_OMISSIONS = 0.01


def sha256(data):
    return hashlib.sha256(data).hexdigest()


# ---------------------------------------------------------------- descriptors

def load(name, refreeze=False):
    """A scenario's descriptor; `refreeze` drops its frozen work, which the
    caller is about to replace."""
    if name not in NAMES:
        raise ValueError(f'unknown scenario {name!r}; scenarios: {", ".join(NAMES)}')
    descriptor = json.loads((SCENARIOS / f'{name}.json').read_text())
    if refreeze:
        descriptor.pop('work', None)
    validate(descriptor, name)
    return descriptor


def validate(descriptor, name):
    keys = {'version', 'name', 'source', 'install', 'project', 'modes'}
    if not isinstance(descriptor, dict) or not keys <= set(descriptor) <= keys | {'work', 'notes', 'oracle_emit_varies'}:
        raise ValueError(f'{name}: descriptor keys must be {sorted(keys)} plus optional work, notes and oracle_emit_varies')
    if descriptor.get('oracle_emit_varies', True) is not True or 'oracle_emit_varies' in descriptor and 'notes' not in descriptor:
        raise ValueError(f'{name}: oracle_emit_varies is only ever true, and its notes say why')
    if descriptor['version'] != 1 or descriptor['name'] != name:
        raise ValueError(f'{name}: descriptor version must be 1 and its name the file name')
    source = descriptor['source']
    if (not isinstance(source, dict) or set(source) - {'repository', 'commit', 'origin'}
            or not REPOSITORY.match(str(source.get('repository'))) or not COMMIT.match(str(source.get('commit')))):
        raise ValueError(f'{name}: source needs a GitHub repository URL and a 40-digit commit')
    install = descriptor['install']
    if (not isinstance(install, dict) or set(install) - {'image', 'commands', 'environment'}
            or not IMAGE.match(str(install.get('image')))
            or not isinstance(install.get('commands'), list) or not install['commands']
            or any(not isinstance(command, str) or not command for command in install['commands'])
            or any(not isinstance(key, str) or not isinstance(value, str)
                   for key, value in install.get('environment', {}).items())):
        raise ValueError(f'{name}: install needs an image pinned by digest and nonempty shell commands')
    project = descriptor['project']
    if not isinstance(project, str) or Path(project).is_absolute() or '..' in Path(project).parts:
        raise ValueError(f'{name}: project must be a path inside the checkout')
    modes = descriptor['modes']
    if (not isinstance(modes, dict) or set(modes) != set(MODES)
            or any(not isinstance(args, list) or any(not isinstance(arg, str) for arg in args) for args in modes.values())
            or OUT_TOKEN not in modes['emit'] or OUT_TOKEN in modes['check']):
        raise ValueError(f'{name}: modes needs check and emit argument lists, emit writing to {OUT_TOKEN}')
    if 'work' in descriptor:
        work = descriptor['work']
        if not isinstance(work, dict) or set(work) != set(MODES):
            raise ValueError(f'{name}: work must describe both modes')
        for mode, facts in work.items():
            expected = WORK_FIELDS | (EMIT_FIELDS if mode == 'emit' else set())
            optional = EMIT_OPTIONAL if mode == 'emit' else set()
            if not isinstance(facts, dict) or not expected <= set(facts) <= expected | optional:
                raise ValueError(f'{name}: work.{mode} fields must be {sorted(expected)}')
            every = facts.get('emitted_in_every_run', 0)
            if type(every) is not int or every < 0 or 'emitted' in facts and every > facts['emitted']:
                raise ValueError(f'{name}: work.{mode}.emitted_in_every_run must be a count within emitted')
            for field in ('inputs_sha256', 'options_sha256', 'output_sha256', *(('emitted_sha256',) if mode == 'emit' else ())):
                if not SHA.match(str(facts[field])):
                    raise ValueError(f'{name}: work.{mode}.{field} must be a sha256')
            for field in ('checkers', 'files', 'listed', 'diagnostics', 'types', *(('emitted', 'oracle_runs') if mode == 'emit' else ())):
                if type(facts[field]) is not int or facts[field] < 0:
                    raise ValueError(f'{name}: work.{mode}.{field} must be a count')
            if type(facts['exit']) is not int:
                raise ValueError(f'{name}: work.{mode}.exit must be an exit status')
            if mode == 'emit' and facts['emitted'] == 0:
                raise ValueError(f'{name}: the oracle emitted nothing in emit mode')
        if ('emitted_in_every_run' in work['emit']) != bool(descriptor.get('oracle_emit_varies')):
            raise ValueError(f'{name}: work.emit.emitted_in_every_run is recorded exactly when oracle_emit_varies is set')
        # Checking plus emit must check: an options error, for one, makes the
        # compiler emit without checking.
        if work['emit']['types'] < TYPES_FLOOR * work['check']['types']:
            raise ValueError(f"{name}: emit mode checked {work['emit']['types']} types against check mode's "
                             f"{work['check']['types']}: its arguments stop checking")


def checkout(descriptor):
    return CACHE / f"{descriptor['name']}-{descriptor['source']['commit'][:12]}"


def provision_spec(descriptor):
    """What the provisioned checkout depends on: a change re-provisions it."""
    spec = {key: descriptor[key] for key in ('source', 'install')}
    return sha256(json.dumps(spec, sort_keys=True).encode())


def changed_files(descriptor):
    """Files outside node_modules written after provisioning: a compiler run
    must leave the checkout as provisioned (build info, for one, would make
    the next run incremental)."""
    root = checkout(descriptor)
    since = (root / MARKER).stat().st_mtime
    changed = []
    for directory, subdirectories, files in os.walk(root):
        subdirectories[:] = [name for name in subdirectories if name != 'node_modules']
        changed += [str(Path(directory, name).relative_to(root)) for name in files
                    if name != MARKER and Path(directory, name).lstat().st_mtime > since]
    return sorted(changed)


def require_unchanged(descriptor):
    changed = changed_files(descriptor)
    if changed:
        raise ValueError(f"{descriptor['name']}: files changed since provisioning (re-provision with --clean): "
                         + ', '.join(changed[:10]))


def provisioned(descriptor):
    marker = checkout(descriptor) / MARKER
    if not marker.is_file():
        return False
    return json.loads(marker.read_text()).get('spec_sha256') == provision_spec(descriptor)


# ---------------------------------------------------------------- provisioning

def archive(descriptor, log):
    owner, repository = REPOSITORY.match(descriptor['source']['repository']).groups()
    commit = descriptor['source']['commit']
    path = CACHE / 'archives' / f"{descriptor['name']}-{commit}.tar.gz"
    if not path.is_file():
        path.parent.mkdir(parents=True, exist_ok=True)
        url = f'https://codeload.github.com/{owner}/{repository}/tar.gz/{commit}'
        print(f'downloading {url}', file=log, flush=True)
        partial = path.with_suffix('.partial')
        with urllib.request.urlopen(url, timeout=600) as response, partial.open('wb') as stream:
            shutil.copyfileobj(response, stream)
        partial.rename(path)
    return path


def extract(path, destination):
    with tarfile.open(path) as stream:
        members = stream.getmembers()
        tops = {Path(member.name).parts[0] for member in members if member.name}
        if len(tops) != 1:
            raise ValueError(f'{path.name}: expected one top-level directory, found {sorted(tops)}')
        top = tops.pop()
        with tempfile.TemporaryDirectory(dir=destination.parent) as directory:
            stream.extractall(directory, filter='data')
            (Path(directory) / top).rename(destination)


def install(descriptor, directory, log):
    settings = descriptor['install']
    environment = {'HOME': '/tmp/home', 'COREPACK_ENABLE_DOWNLOAD_PROMPT': '0',
                   'npm_config_update_notifier': 'false', **settings.get('environment', {})}
    script = 'set -ex\n' + '\n'.join(settings['commands'])
    command = ['docker', 'run', '--rm', '-u', f'{os.getuid()}:{os.getgid()}']
    for key, value in sorted(environment.items()):
        command += ['-e', f'{key}={value}']
    command += ['-v', f'{directory}:/work', '-w', '/work', settings['image'], 'sh', '-c', script]
    subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT)


def provision(descriptor, clean=False, log=sys.stdout):
    """Download, extract and install one scenario; a matching marker skips it."""
    directory = checkout(descriptor)
    if provisioned(descriptor) and not clean:
        print(f"{descriptor['name']}: provisioned at {directory}", file=log, flush=True)
        return directory
    if directory.exists():
        shutil.rmtree(directory)
    directory.parent.mkdir(parents=True, exist_ok=True)
    path = archive(descriptor, log)
    started = time.time()
    extract(path, directory)
    install(descriptor, directory, log)
    marker = {'spec_sha256': provision_spec(descriptor), 'archive': path.name,
              'archive_sha256': sha256(path.read_bytes()), 'seconds': round(time.time() - started)}
    (directory / MARKER).write_text(json.dumps(marker, indent=2) + '\n')
    print(f"{descriptor['name']}: provisioned at {directory} in {marker['seconds']}s", file=log, flush=True)
    return directory


# ---------------------------------------------------------------- binaries

def go_environment():
    return {**os.environ, 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOFLAGS': '-mod=readonly',
            'GOCACHE': os.environ.get('GOCACHE', str(ROOT / 'target/go-build'))}


def pinned_go():
    expected = tomllib.loads((ROOT / 'data/s04/toolchains.toml').read_text())['go']
    candidates = [shutil.which('go'),
                  str(Path.home() / '.local/share/mise/installs/go' / expected.removeprefix('go') / 'bin/go')]
    for candidate in dict.fromkeys(filter(None, candidates)):
        if Path(candidate).is_file():
            version = subprocess.check_output([candidate, 'env', 'GOVERSION'], env=go_environment(), text=True).strip()
            if version == expected:
                return str(Path(candidate).resolve())
    raise ValueError(f'put the pinned {expected} on PATH (automatic Go downloads are disabled)')


def build(log=sys.stdout):
    """The two ordinary release binaries: tsgo from the pin, tsrust from HEAD."""
    BINARIES.mkdir(parents=True, exist_ok=True)
    subprocess.run([pinned_go(), 'build', '-trimpath', '-o', str(BINARIES / 'tsgo'), './cmd/tsc'],
                   cwd=ROOT / 'upstream/tsc', env=go_environment(), check=True, stdout=log, stderr=subprocess.STDOUT)
    result = subprocess.run(['cargo', 'build', '--release', '--locked', '-p', 'tsrust', '--bin', 'tsrust',
                             '--message-format=json-render-diagnostics'],
                            cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    executables = [item['executable'] for item in map(json.loads, result.stdout.splitlines())
                   if item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'tsrust'
                   and item.get('executable')]
    if len(executables) != 1:
        raise ValueError(f'expected one tsrust executable, got {executables}')
    shutil.copy2(executables[0], BINARIES / 'tsrust')
    return binaries()


def binaries():
    found = {'go': BINARIES / 'tsgo', 'rust': BINARIES / 'tsrust'}
    missing = [str(path) for path in found.values() if not path.is_file()]
    if missing:
        raise ValueError('build the binaries first (tools/phase7/bench/capture.py build): ' + ', '.join(missing))
    return found


def identity(path):
    return {'path': str(path), 'sha256': sha256(Path(path).read_bytes()), 'bytes': Path(path).stat().st_size}


# ---------------------------------------------------------------- one run

def arguments(descriptor, mode, checkers, out=None, extra=()):
    args = list(descriptor['modes'][mode])
    if mode == 'emit':
        if out is None:
            raise ValueError('emit mode needs an output directory')
        args = [str(out) if arg == OUT_TOKEN else arg for arg in args]
    return ['-p', descriptor['project'], *args, '--pretty', 'false', '--checkers', str(checkers),
            '--listFiles', '--extendedDiagnostics', *extra]


def run(binary, cwd, args, timeout=TIMEOUT_SECONDS):
    """One child process: exit status, output, wall time from launch to exit,
    and its own peak RSS and CPU times from wait4."""
    if sys.platform not in ('darwin', 'linux'):
        raise ValueError('peak RSS is normalized for macOS and Linux only')
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        child = subprocess.Popen([str(binary), *args], cwd=cwd, stdout=stdout, stderr=stderr)
        expired = threading.Event()

        def expire():
            expired.set()
            child.send_signal(signal.SIGKILL)

        timer = threading.Timer(timeout, expire)
        timer.start()
        try:
            _, status, usage = os.wait4(child.pid, 0)
        finally:
            timer.cancel()
        wall = time.perf_counter_ns() - started
        child.returncode = os.waitstatus_to_exitcode(status)
        if expired.is_set():
            raise ValueError(f'{Path(binary).name} exceeded {timeout}s')
        stdout.seek(0)
        stderr.seek(0)
        return {'exit': child.returncode, 'stdout': stdout.read().decode('utf-8', errors='replace'),
                'stderr': stderr.read().decode('utf-8', errors='replace'), 'wall_ns': wall,
                'peak_rss_bytes': usage.ru_maxrss * (1024 if sys.platform == 'linux' else 1),
                'user_ns': round(usage.ru_utime * 1e9), 'system_ns': round(usage.ru_stime * 1e9)}


def split_output(stdout):
    """The printed output (diagnostics and file list) and the statistics table
    that --extendedDiagnostics appends, which starts at its `Files:` row."""
    lines = stdout.splitlines()
    for index, line in enumerate(lines):
        if line.startswith('Files:') and all(STATISTIC.match(rest) for rest in lines[index:] if rest):
            statistics = {}
            for rest in lines[index:]:
                if rest:
                    key, value = STATISTIC.match(rest).groups()
                    statistics[key] = value
            return lines[:index], statistics
    raise ValueError('the run printed no --extendedDiagnostics statistics')


def statistic_count(statistics, key):
    value = statistics.get(key)
    if value is None or not value.isdigit():
        raise ValueError(f'statistic {key!r} is missing or not a count: {value!r}')
    return int(value)


def statistic_seconds(statistics, key):
    value = statistics.get(key)
    if value is None:
        return None
    if not re.fullmatch(r'\d+(\.\d+)?s', value):
        raise ValueError(f'statistic {key!r} is not a duration: {value!r}')
    return float(value[:-1])


def tree_files(directory):
    """Every file under `directory`: relative path -> sha256 of its bytes."""
    return {path.relative_to(directory).as_posix(): sha256(path.read_bytes())
            for path in sorted(Path(directory).rglob('*')) if path.is_file()}


def files_digest(files):
    return len(files), sha256(''.join(f'{path}\t{files[path]}\n' for path in sorted(files)).encode())


def observe(result, root, out=None):
    """The comparable facts of one run (paths relative to the checkout)."""
    printed, statistics = split_output(result['stdout'])
    prefix = str(root) + '/'
    normalized = [line.replace(prefix, ROOT_TOKEN + '/') for line in printed]
    listed = [line for line in printed if line.startswith(prefix) or line.startswith('bundled://')]
    facts = {
        'exit': result['exit'],
        'output_sha256': sha256(('\n'.join(normalized) + '\n').encode()),
        'diagnostics': sum(1 for line in printed if DIAGNOSTIC.match(line)),
        'listed': len(listed),
        'files': statistic_count(statistics, 'Files'),
        'types': statistic_count(statistics, 'Types'),
        'check_seconds': statistic_seconds(statistics, 'Check time'),
        'emit_seconds': statistic_seconds(statistics, 'Emit time'),
        'total_seconds': statistic_seconds(statistics, 'Total time'),
        'statistics': statistics,
        'listed_paths': listed,
    }
    if out is not None:
        facts['emitted_files'] = tree_files(out)
        facts['emitted'], facts['emitted_sha256'] = files_digest(facts['emitted_files'])
    return facts


def inputs_digest(root, listed):
    """The listed files and their bytes; bundled library files by name (the
    pin fixes their bytes)."""
    prefix = str(root) + '/'
    entries = []
    for path in listed:
        if path.startswith(prefix):
            entries.append(f'{path[len(prefix):]}\t{sha256(Path(path).read_bytes())}\n')
        else:
            entries.append(f'{path}\t-\n')
    return sha256(''.join(entries).encode())


def options_digest(binary, descriptor, mode, root):
    """The oracle's --showConfig of the mode's arguments, root-relative."""
    # --showConfig writes nothing and prints outDir relative to the project,
    # so the probe names one fixed directory that is never created.
    out = CACHE / 'emit' / 'show-config'
    args = [arg for arg in arguments(descriptor, mode, ORACLE_CHECKERS, out)
            if arg not in ('--listFiles', '--extendedDiagnostics')] + ['--showConfig']
    result = run(binary, root, args)
    if result['exit'] != 0:
        raise ValueError(f"{descriptor['name']} {mode}: --showConfig exited {result['exit']}: {result['stderr'][:500]}")
    text = result['stdout'].replace(str(root) + '/', ROOT_TOKEN + '/')
    return sha256(text.encode()), text


def problems(facts, work, mode, runtime='rust', reference=None):
    """Every way one run's facts differ from the frozen work of its mode.

    Emitted files must be the oracle's union exactly, except that the oracle
    runtime itself, when its emit varies, may omit up to ORACLE_OMISSIONS of
    them; every file it does emit must have the bytes of `reference`, a run
    whose files are the whole union.
    """
    found = []
    for field in ('exit', 'output_sha256', 'files', 'listed', 'diagnostics'):
        if facts[field] != work[field]:
            found.append(f'{field} {facts[field]!r}, the oracle {work[field]!r}')
    if not facts['check_seconds']:
        found.append('no checking time: the checker did not run')
    if facts['types'] < TYPES_FLOOR * work['types']:
        found.append(f"{facts['types']} types against the oracle's {work['types']}: checking work was skipped")
    if mode == 'emit':
        files = facts.get('emitted_files', {})
        count, digest = files_digest(files) if files else (facts.get('emitted'), facts.get('emitted_sha256'))
        if (count, digest) == (work['emitted'], work['emitted_sha256']):
            return found
        if runtime != 'go' or 'emitted_in_every_run' not in work:
            found.append(f"emitted {count} files ({digest}), the oracle {work['emitted']} ({work['emitted_sha256']})")
        elif reference is None:
            found.append(f"emitted {count} of the oracle's {work['emitted']} files and no run emitted all of them")
        else:
            changed = sorted(path for path, sha in files.items() if reference.get(path) != sha)
            omitted = len(reference) - (len(files) - len(changed))
            if changed:
                found.append(f'{len(changed)} emitted files differ from the oracle\'s, for one {changed[0]}')
            if omitted > ORACLE_OMISSIONS * len(reference):
                found.append(f'omitted {omitted} of the oracle\'s {len(reference)} files')
    return found


def emit_directory():
    """A fresh emit directory beside the checkouts, never inside a project."""
    (CACHE / 'emit').mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix='out-', dir=CACHE / 'emit'))
